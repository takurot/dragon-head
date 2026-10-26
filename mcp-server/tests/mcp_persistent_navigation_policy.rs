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
    anyhow::ensure!(
        !test_bench_support::should_skip_browser_tests(),
        "Chrome is required"
    );
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
    anyhow::ensure!(
        !test_bench_support::should_skip_browser_tests(),
        "Chrome is required"
    );
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
    anyhow::ensure!(
        !test_bench_support::should_skip_browser_tests(),
        "Chrome is required"
    );
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
    anyhow::ensure!(
        !test_bench_support::should_skip_browser_tests(),
        "Chrome is required"
    );
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
    anyhow::ensure!(
        !test_bench_support::should_skip_browser_tests(),
        "Chrome is required"
    );
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
    anyhow::ensure!(
        !test_bench_support::should_skip_browser_tests(),
        "Chrome is required"
    );
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
    anyhow::ensure!(
        !test_bench_support::should_skip_browser_tests(),
        "Chrome is required"
    );
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
