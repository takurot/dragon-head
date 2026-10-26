#[path = "common/navigation_fixture.rs"]
mod navigation_fixture;
use core_runtime::BrowserClient;
use mcp_server::{CoreRuntimeBackend, McpServer};
use navigation_fixture::PrivateDestination;
use serde_json::{json, Value};
use std::{
    sync::atomic::Ordering,
    thread,
    time::{Duration, Instant},
};

fn source_html(kind: &str, url: &str) -> String {
    let target = serde_json::to_string(url).unwrap();
    let action = match kind {
        "link" => format!("<a href='{url}'>go private</a>"),
        "form" => format!("<form action='{url}'><button>submit private</button></form>"),
        "script" => format!("<button onclick='location.href={target}'>script private</button>"),
        "meta" => format!("<button onclick='const meta=document.createElement(\"meta\"); meta.httpEquiv=\"refresh\"; meta.content=\"0;url=\"+{target}; document.head.append(meta)'>meta private</button>"),
        _ => unreachable!(),
    };
    format!("<html><head></head><body>{action}</body></html>")
}

fn server_for_html(
    html: &str,
    allow_private: bool,
) -> anyhow::Result<McpServer<CoreRuntimeBackend>> {
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    page.navigate(&format!("data:text/html,{}", urlencoding::encode(html)))?;
    let mut backend = CoreRuntimeBackend::new_with_client(client, page);
    backend.set_navigation_allow_private_network(allow_private);
    Ok(McpServer::new(backend))
}

fn click_first(server: &mut McpServer<CoreRuntimeBackend>) -> anyhow::Result<Value> {
    let state = server.call_tool("get_state", json!({}))?;
    let id = state["interactive_elements"][0]["id"]
        .as_i64()
        .expect("interactive target");
    server.call_tool("act", json!({"action": "click", "target_id": id}))
}

#[test]
fn persistent_boundary_blocks_link_form_script_and_meta_before_private_io() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    for kind in ["link", "form", "script", "meta"] {
        let destination = PrivateDestination::start()?;
        let mut server = server_for_html(&source_html(kind, &destination.url), false)?;
        let result = click_first(&mut server);
        assert!(
            !result.as_ref().is_ok_and(|result| result["status"] == "ok"),
            "{kind}: {result:?}"
        );
        assert_eq!(
            destination.count.load(Ordering::Acquire),
            0,
            "{kind}: private request escaped"
        );
    }
    Ok(())
}

#[test]
fn persistent_boundary_private_opt_in_allows_the_same_actions() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    for kind in ["link", "form", "script", "meta"] {
        let destination = PrivateDestination::start()?;
        let mut server = server_for_html(&source_html(kind, &destination.url), true)?;
        assert_eq!(click_first(&mut server)?["status"], "ok", "{kind}");
        let deadline = Instant::now() + Duration::from_secs(3);
        while destination.count.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            destination.count.load(Ordering::Acquire) > 0,
            "{kind}: opted-in navigation did not happen"
        );
    }
    Ok(())
}

#[test]
fn allowed_autonomous_navigation_invalidates_cached_full_and_delta_state() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let destination = PrivateDestination::start()?;
    let mut server = server_for_html(
        "<html><body><button>INITIAL DOCUMENT</button></body></html>",
        true,
    )?;
    let original = server.call_tool("get_state", json!({}))?;
    server.backend_mut().page().evaluate_script(&format!(
        "location.href = {}",
        serde_json::to_string(&destination.url)?
    ))?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while server.backend_mut().page().current_url()? != destination.url && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(10));
    }
    let delta = server.call_tool("get_state", json!({"delivery": "delta"}))?;
    assert_eq!(
        delta["type"], "full",
        "new document must establish a fresh baseline: {delta}"
    );
    assert_eq!(delta["state"]["metadata"]["url"], destination.url);
    let next_url = format!("{}?next", destination.url);
    server.backend_mut().page().evaluate_script(&format!(
        "location.href = {}",
        serde_json::to_string(&next_url)?
    ))?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while server.backend_mut().page().current_url()? != next_url && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let current = server.call_tool("get_state", json!({}))?;
    assert_ne!(current["metadata"]["url"], original["metadata"]["url"]);
    assert_eq!(current["metadata"]["url"], next_url);
    Ok(())
}

#[test]
fn delayed_navigation_is_blocked_and_reported_once_before_the_next_read() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let destination = PrivateDestination::start()?;
    let target = serde_json::to_string(&destination.url)?;
    let html = format!("<html><body><button onclick='setTimeout(() => location.href={target}, 250)'>delayed</button></body></html>");
    let mut server = server_for_html(&html, false)?;
    assert_eq!(click_first(&mut server)?["status"], "ok");
    // This timer fires after the bounded action settlement. Fetch must still
    // block before network I/O, and the following read reports that rejection.
    thread::sleep(Duration::from_millis(400));
    assert_eq!(destination.count.load(Ordering::Acquire), 0);
    let rejected = server.call_tool("get_visual", json!({"mode": "clean"}));
    assert!(rejected.is_err(), "late rejection was hidden: {rejected:?}");
    server
        .backend_mut()
        .page()
        .navigate("data:text/html,%3Cbutton%3Esafe%3C/button%3E")?;
    assert_eq!(
        click_first(&mut server)?["status"],
        "ok",
        "a consumed rejection poisoned the next action"
    );
    assert_eq!(destination.count.load(Ordering::Acquire), 0);
    Ok(())
}

#[test]
fn delayed_navigation_rejection_precedes_an_unrelated_action() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let destination = PrivateDestination::start()?;
    let target = serde_json::to_string(&destination.url)?;
    let html = format!("<html><body><button onclick='setTimeout(() => location.href={target}, 250)'>delayed</button><button onclick='window.name=\"safe-action-executed\"'>safe</button></body></html>");
    let mut server = server_for_html(&html, false)?;
    let state = server.call_tool("get_state", json!({}))?;
    let delayed = state["interactive_elements"][0]["id"].as_i64().unwrap();
    let safe = state["interactive_elements"][1]["id"].as_i64().unwrap();
    assert_eq!(
        server.call_tool("act", json!({"action": "click", "target_id": delayed}))?["status"],
        "ok"
    );
    thread::sleep(Duration::from_millis(400));
    let result = server.call_tool("act", json!({"action": "click", "target_id": safe}));
    let error = result.expect_err("pending navigation denial must precede the unrelated action");
    assert!(
        matches!(
            error.downcast_ref::<core_runtime::ActionError>(),
            Some(core_runtime::ActionError::Blocked { rule_id }) if rule_id == "navigation-network-policy"
        ),
        "unexpected error instead of pending navigation denial: {error:?}"
    );
    assert_eq!(destination.count.load(Ordering::Acquire), 0);
    assert_eq!(
        server
            .backend_mut()
            .page()
            .evaluate_script_json("window.name")?,
        json!("")
    );
    // Chrome may replace the source with an error document after blocking
    // navigation. Establish a fresh safe target after consuming the rejection.
    let safe_html = r#"<button onclick='window.name="safe-action-executed"'>safe</button>"#;
    server.backend_mut().page().navigate(&format!(
        "data:text/html,{}",
        urlencoding::encode(safe_html)
    ))?;
    assert_eq!(click_first(&mut server)?["status"], "ok");
    assert_eq!(
        server
            .backend_mut()
            .page()
            .evaluate_script_json("window.name")?,
        json!("safe-action-executed")
    );
    Ok(())
}

#[test]
fn explicit_navigation_expires_preexisting_timeboxed_action_approval() -> anyhow::Result<()> {
    use core_runtime::{
        policy::{ApprovalScope, PolicyAction, PolicyRule},
        sre::{LoadProfile, SemanticNode},
        ActionError,
    };
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    fn button_id(node: &SemanticNode) -> Option<i64> {
        if node.role == "button" {
            Some(node.backend_node_id)
        } else {
            node.children.iter().find_map(button_id)
        }
    }
    let destination = PrivateDestination::start()?;
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    let html = "<html><body><button>PRIVATE DESTINATION</button></body></html>";
    page.navigate(&format!("data:text/html,{}", urlencoding::encode(html)))?;
    page.set_policy_rules(vec![PolicyRule {
        id: "approve-button".to_string(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_string()),
        text_regex: None,
        context_regex: None,
        action: PolicyAction::RequireHumanApproval,
        scope: Some(ApprovalScope::Timeboxed { ms: 60_000 }),
        outcome_projector: None,
    }])?;
    let first = page.capture_semantic_state(LoadProfile::Interactive)?;
    let first_id = button_id(first.root()).unwrap();
    assert!(page.act(Some(first_id), None, "click", None).is_err());
    page.approve_pending_policy_action()?;
    page.act(Some(first_id), None, "click", None)?;
    page.navigate_public(&destination.url, true)?;
    let second = page.capture_semantic_state(LoadProfile::Interactive)?;
    let second_id = button_id(second.root()).unwrap();
    let error = page
        .act(Some(second_id), None, "click", None)
        .expect_err("explicit navigation must expire the previous document's approval");
    assert!(matches!(
        error.downcast_ref::<ActionError>(),
        Some(ActionError::HumanApprovalRequired { .. })
    ));
    Ok(())
}

#[cfg(unix)]
#[test]
fn invalid_navigation_is_rejected_before_disconnected_browser_io() -> anyhow::Result<()> {
    use core_runtime::NavigationValidationError;
    use mcp_server::McpBackend;
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    let pid = client.process_id().expect("managed Chrome process");
    let mut server = McpServer::new(CoreRuntimeBackend::new_with_client(client, page));
    let status = std::process::Command::new("kill")
        .arg("-9")
        .arg(pid.to_string())
        .status()?;
    anyhow::ensure!(status.success(), "failed to kill managed Chrome");
    let deadline = Instant::now() + Duration::from_secs(3);
    while server.backend_mut().is_chrome_process_alive() != Some(false) && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(server.backend_mut().is_chrome_process_alive(), Some(false));
    for (url, reason) in [
        (
            "file:///private/secret",
            NavigationValidationError::InvalidUrl,
        ),
        (
            "http://user:secret@example.com/",
            NavigationValidationError::CredentialsNotAllowed,
        ),
        (
            "http://127.0.0.1/private",
            NavigationValidationError::NonPublicAddress,
        ),
    ] {
        let error = server
            .call_tool("navigate", json!({"url": url}))
            .expect_err("invalid URL must fail before touching the disconnected browser");
        assert!(
            error.to_string().contains(&reason.to_string()),
            "expected validation reason {reason}, got {error:?}"
        );
        assert_eq!(
            server.backend_mut().browser_restart_count(),
            0,
            "invalid URL must not recover the disconnected browser"
        );
        if reason != NavigationValidationError::NonPublicAddress {
            assert_eq!(
                error.downcast_ref::<NavigationValidationError>(),
                Some(&reason)
            );
        }
    }
    Ok(())
}

#[test]
fn public_navigation_waits_for_a_slow_parser_blocking_script() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let destination = PrivateDestination::start()?;
    let slow_url = format!(
        "{}/slow-page",
        destination.url.strip_suffix("/private").unwrap()
    );
    let mut server = server_for_html("<button>initial</button>", true)?;
    let started = Instant::now();
    let result = server.call_tool("navigate", json!({"url": slow_url}))?;
    assert_eq!(result["status"], "ok");
    assert_eq!(result["final_url"], slow_url);
    assert!(started.elapsed() >= Duration::from_secs(4));
    assert_eq!(
        server
            .backend_mut()
            .page()
            .evaluate_script_json("window.slowScriptLoaded === true")?,
        json!(true)
    );
    Ok(())
}

#[test]
fn public_navigation_waits_for_deferred_initialization_before_state_capture() -> anyhow::Result<()>
{
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let destination = PrivateDestination::start()?;
    let url = format!(
        "{}/deferred-page",
        destination.url.strip_suffix("/private").unwrap()
    );
    let mut server = server_for_html("<button>initial</button>", true)?;
    let result = server.call_tool("navigate", json!({"url": url}))?;
    assert_eq!(result["status"], "ok");
    let state = server.call_tool("get_state", json!({}))?;
    assert!(
        state.to_string().contains("DEFER INITIALIZATION COMPLETE"),
        "navigation returned before deferred initialization; immediate state: {state}"
    );
    Ok(())
}

#[test]
fn delayed_navigation_hitl_preserves_response_ticket_and_usage() -> anyhow::Result<()> {
    use core_runtime::policy::{ApprovalScope, OutcomeProjectorConfig, PolicyAction, PolicyRule};
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    for trigger in ["act", "get_state", "get_visual"] {
        let destination = PrivateDestination::start()?;
        let target = serde_json::to_string(&destination.url)?;
        let html = format!("<button onclick='setTimeout(() => location.href={target}, 500)'>delayed</button><button onclick='window.name=\"unrelated-action-executed\"'>safe</button>");
        let mut server = server_for_html(&html, true)?;
        server
            .backend_mut()
            .page()
            .set_policy_rules(vec![PolicyRule {
                id: "approve-delayed-navigation".to_string(),
                domain: None,
                path_prefix: Some("/private".to_string()),
                role: None,
                text_regex: None,
                context_regex: None,
                action: PolicyAction::RequireHumanApproval,
                scope: Some(ApprovalScope::ActionOnly),
                outcome_projector: Some(OutcomeProjectorConfig {
                    amount_regex: None,
                    block_if_amount_exceeds: None,
                    warn_if_amount_exceeds: None,
                }),
            }])?;
        let state = server.call_tool("get_state", json!({}))?;
        let delayed = state["interactive_elements"][0]["id"].as_i64().unwrap();
        let safe = state["interactive_elements"][1]["id"].as_i64().unwrap();
        if trigger == "get_visual" {
            // Leave an earlier backend image unconsumed to verify that the
            // approval response cannot attach stale visual bytes.
            let stale =
                mcp_server::McpBackend::get_visual(server.backend_mut(), json!({"mode": "som"}))?;
            assert!(stale.get("image_sha256").is_some());
        }
        assert_eq!(
            server.call_tool("act", json!({"action": "click", "target_id": delayed}))?["status"],
            "ok"
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while server
            .backend_mut()
            .page()
            .pending_policy_approval_snapshot()
            .is_none()
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        let pending = server
            .backend_mut()
            .page()
            .pending_policy_approval_snapshot()
            .expect("delayed navigation approval");
        assert_eq!(pending.request.action, "navigate");
        let arguments = match trigger {
            "act" => json!({"action": "click", "target_id": safe}),
            "get_visual" => json!({"mode": "som"}),
            _ => json!({}),
        };
        let request = json!({"jsonrpc": "2.0", "id": 99, "method": "tools/call", "params": {"name": trigger, "arguments": arguments}});
        let response: Value =
            serde_json::from_str(&server.handle_jsonrpc(&request.to_string()).unwrap())?;
        assert!(
            response.get("error").is_none(),
            "{trigger}: unexpected RPC error: {response}"
        );
        let payload = &response["result"]["structuredContent"];
        assert_eq!(payload["status"], "requires_human_approval", "{trigger}");
        assert_eq!(payload["rule_id"], "approve-delayed-navigation");
        assert_eq!(payload["scope"], "action_only");
        assert_eq!(
            payload["outcome_projection"],
            serde_json::to_value(pending.request.outcome.as_ref().unwrap())?
        );
        assert_eq!(
            response["result"]["content"].as_array().unwrap().len(),
            1,
            "HITL must not attach a visual image"
        );
        assert_eq!(
            server
                .backend_mut()
                .page()
                .pending_policy_approval_snapshot()
                .unwrap()
                .id,
            pending.id
        );
        assert_eq!(destination.count.load(Ordering::Acquire), 0);
        assert_eq!(
            server
                .backend_mut()
                .page()
                .evaluate_script_json("window.name")?,
            json!("")
        );
        let usage = server.call_tool("get_usage_report", json!({}))?;
        assert_eq!(usage["hitl_events"], 1, "{trigger}");
        assert_eq!(
            usage["actions_executed"], 1,
            "unrelated action must not execute"
        );
        assert_eq!(
            usage["state_generations"]["full"], 1,
            "approval response is not a state capture"
        );
        assert_eq!(usage["visual_captures"], 0);
        assert_eq!(
            server.call_tool("get_usage_report", json!({}))?["hitl_events"],
            1
        );
        assert_eq!(
            server.call_tool("ask_human", json!({"reason": "review delayed navigation"}))?
                ["approved"],
            true
        );
        assert_eq!(
            server.call_tool("navigate", json!({"url": destination.url}))?["status"],
            "ok"
        );
        assert_eq!(destination.count.load(Ordering::Acquire), 1);
        assert!(server
            .backend_mut()
            .page()
            .pending_policy_approval_snapshot()
            .is_none());
        let usage = server.call_tool("get_usage_report", json!({}))?;
        assert_eq!(
            usage["hitl_events"], 2,
            "request and approval are separate events"
        );
        assert_eq!(usage["actions_executed"], 2);
    }
    Ok(())
}

#[test]
fn independent_automatic_navigation_approval_is_retryable_after_explicit_navigation(
) -> anyhow::Result<()> {
    use core_runtime::policy::{ApprovalScope, PolicyAction, PolicyRule};
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let destination = PrivateDestination::start()?;
    let source = format!(
        "{}/source",
        destination.url.strip_suffix("/private").unwrap()
    );
    let mut server = server_for_html("<button>initial</button>", true)?;
    server
        .backend_mut()
        .page()
        .set_policy_rules(vec![PolicyRule {
            id: "approve-independent-navigation".to_string(),
            domain: None,
            path_prefix: Some("/private".to_string()),
            role: None,
            text_regex: None,
            context_regex: None,
            action: PolicyAction::RequireHumanApproval,
            scope: Some(ApprovalScope::ActionOnly),
            outcome_projector: None,
        }])?;
    assert_eq!(
        server.call_tool("navigate", json!({"url": source}))?["status"],
        "ok"
    );
    // Schedule an independent page timer only after the explicit tool completed.
    server.backend_mut().page().evaluate_script(&format!(
        "setTimeout(() => location.href = {}, 100)",
        serde_json::to_string(&destination.url)?
    ))?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while server
        .backend_mut()
        .page()
        .pending_policy_approval_snapshot()
        .is_none()
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(10));
    }
    let pending = server
        .backend_mut()
        .page()
        .pending_policy_approval_snapshot()
        .expect("independent navigation approval");
    assert_eq!(destination.count.load(Ordering::Acquire), 0);
    assert_eq!(
        server.call_tool("get_state", json!({}))?["status"],
        "requires_human_approval"
    );
    assert_eq!(
        server
            .backend_mut()
            .page()
            .pending_policy_approval_snapshot()
            .unwrap()
            .id,
        pending.id
    );
    assert_eq!(
        server.call_tool(
            "ask_human",
            json!({"reason": "approve independent navigation"})
        )?["approved"],
        true
    );
    let retry = server.call_tool("navigate", json!({"url": destination.url}))?;
    assert_eq!(
        retry["status"], "ok",
        "approved independent destination must be directly retryable: {retry}"
    );
    assert_eq!(destination.count.load(Ordering::Acquire), 1);
    assert!(server
        .backend_mut()
        .page()
        .pending_policy_approval_snapshot()
        .is_none());
    let usage = server.call_tool("get_usage_report", json!({}))?;
    assert_eq!(usage["hitl_events"], 2);
    assert_eq!(usage["actions_executed"], 2);
    Ok(())
}
