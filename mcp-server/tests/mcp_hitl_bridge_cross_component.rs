//! ISSUE-302: a policy approval raised inside a `CoreRuntimeBackend`/`McpServer` (the same
//! object graph `dragon-head-mcp`'s `main.rs` builds) must be observable and resolvable through
//! `hitl_bridge::gateway::PageSessionGateway` built from `CoreRuntimeBackend::page_handle()` —
//! not only through the backend's own `ask_human` tool. This is what makes the embedded bridge
//! in `mcp_server::hitl::spawn_embedded_bridge` share state with the MCP server it runs inside,
//! instead of independently polling an unrelated `PageSession` (the bug the standalone
//! `dragon-head-hitl-bridge` binary had when run alongside `dragon-head-mcp`).
use core_runtime::{ApprovalScope, BrowserClient, PolicyAction, PolicyRule};
use hitl_bridge::gateway::{ApprovalGateway, PageSessionGateway};
use mcp_server::{CoreRuntimeBackend, McpServer};
use serde_json::json;

/// Sets up a page with a policy-gated "purchase" button and returns the (target_id,
/// stable_key) of that button, after navigating and registering the policy rule.
fn setup_gated_button(page: &core_runtime::PageSession) -> anyhow::Result<()> {
    page.set_policy_rules(vec![PolicyRule {
        id: "checkout-approval".to_string(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_string()),
        text_regex: Some("purchase".to_string()),
        context_regex: None,
        action: PolicyAction::RequireHumanApproval,
        scope: Some(ApprovalScope::ActionOnly),
        outcome_projector: None,
    }])?;

    let html = r#"
        <html>
            <body>
                <button id="purchase" onclick="document.body.dataset.clicked='yes'">Purchase</button>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;
    Ok(())
}

fn locate_purchase_button(state: &serde_json::Value) -> (i64, String) {
    let element = state["interactive_elements"]
        .as_array()
        .expect("interactive_elements array")
        .iter()
        .find(|element| {
            element["role"] == json!("button")
                && element["name"]
                    .as_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case("purchase"))
        })
        .expect("purchase button not found in interactive_elements");
    let target_id = element["id"].as_i64().expect("target id");
    let stable_key = element["stable_key"]
        .as_str()
        .expect("stable key")
        .to_string();
    (target_id, stable_key)
}

#[test]
fn bridge_gateway_observes_and_approves_the_mcp_servers_own_pending_approval() -> anyhow::Result<()>
{
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    setup_gated_button(&page)?;

    let mut server = McpServer::new(CoreRuntimeBackend::new(page));

    // Build the bridge's gateway from the *same* backend the MCP server is using, exactly as
    // `mcp_server::hitl::spawn_embedded_bridge` does from `main.rs`.
    let gateway = PageSessionGateway::new(server.backend_mut().page_handle());

    // No action has been attempted yet, so the shared session has nothing pending.
    assert!(
        gateway.pending_request().is_none(),
        "gateway must not report a pending request before any policy-gated action runs"
    );

    let state = server.call_tool(
        "get_state",
        json!({ "format": "json", "force_refresh": true }),
    )?;
    let (target_id, stable_key) = locate_purchase_button(&state);

    let first_act = server.call_tool(
        "act",
        json!({
            "target_id": target_id,
            "target_stable_key": stable_key,
            "action": "click"
        }),
    )?;
    assert_eq!(first_act["status"], json!("requires_human_approval"));

    // The bridge's gateway -- built independently of `CoreRuntimeBackend`'s own `ask_human`
    // path -- must see the exact same pending approval the MCP server just raised.
    let pending = gateway
        .pending_request()
        .expect("bridge gateway must observe the MCP server's own pending approval");
    assert_eq!(pending.rule_id, "checkout-approval");

    // Resolve it through the bridge, not through `ask_human`.
    gateway.approve(pending.id)?;

    // The *original* MCP-owned session must now show no pending approval, and a retry of the
    // gated action must go through -- proving the bridge mutated the exact same approval state
    // `CoreRuntimeBackend` consults, not a copy of it.
    assert!(
        server
            .backend_mut()
            .page()
            .pending_policy_approval()
            .is_none(),
        "approving via the bridge gateway must clear the original MCP session's pending approval"
    );

    let second_act = server.call_tool(
        "act",
        json!({
            "target_id": target_id,
            "target_stable_key": stable_key,
            "action": "click"
        }),
    )?;
    assert_eq!(second_act["status"], json!("ok"));

    let clicked = server
        .backend_mut()
        .page()
        .evaluate_script("document.body.dataset.clicked")?
        .value
        .and_then(|v| v.as_str().map(ToOwned::to_owned));
    assert_eq!(clicked.as_deref(), Some("yes"));

    Ok(())
}

#[test]
fn bridge_gateway_reject_path_clears_the_mcp_servers_own_pending_approval() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    setup_gated_button(&page)?;

    let mut server = McpServer::new(CoreRuntimeBackend::new(page));
    let gateway = PageSessionGateway::new(server.backend_mut().page_handle());

    let state = server.call_tool(
        "get_state",
        json!({ "format": "json", "force_refresh": true }),
    )?;
    let (target_id, stable_key) = locate_purchase_button(&state);

    server.call_tool(
        "act",
        json!({
            "target_id": target_id,
            "target_stable_key": stable_key,
            "action": "click"
        }),
    )?;

    let pending = gateway
        .pending_request()
        .expect("bridge gateway must observe the pending approval");
    gateway.reject(pending.id)?;

    assert!(
        server
            .backend_mut()
            .page()
            .pending_policy_approval()
            .is_none(),
        "rejecting via the bridge gateway must clear the original MCP session's pending approval"
    );

    // The rejected click must never have gone through.
    let clicked = server
        .backend_mut()
        .page()
        .evaluate_script("document.body.dataset.clicked")?
        .value
        .and_then(|v| v.as_str().map(ToOwned::to_owned));
    assert_eq!(clicked, None);

    Ok(())
}

/// A gateway built from an unrelated `PageSession` (mirroring the pre-ISSUE-302 standalone
/// `dragon-head-hitl-bridge` topology) must NOT observe an approval raised in a different,
/// unrelated `CoreRuntimeBackend`'s session -- confirming `page_handle()` sharing (not global
/// state) is what makes the above tests pass.
#[test]
fn gateway_on_an_unrelated_session_does_not_see_another_sessions_pending_approval(
) -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    setup_gated_button(&page)?;
    let mut server = McpServer::new(CoreRuntimeBackend::new(page));

    let state = server.call_tool(
        "get_state",
        json!({ "format": "json", "force_refresh": true }),
    )?;
    let (target_id, stable_key) = locate_purchase_button(&state);
    server.call_tool(
        "act",
        json!({
            "target_id": target_id,
            "target_stable_key": stable_key,
            "action": "click"
        }),
    )?;
    assert!(server
        .backend_mut()
        .page()
        .pending_policy_approval()
        .is_some());

    // A second, wholly independent client/page/backend -- the pre-fix standalone-bridge shape.
    let other_client = BrowserClient::new()?;
    let other_page = other_client.new_page()?;
    other_page.navigate("data:text/html,<html><body>unrelated</body></html>")?;
    let unrelated_backend = CoreRuntimeBackend::new(other_page);
    let unrelated_gateway = PageSessionGateway::new(unrelated_backend.page_handle());

    assert!(
        unrelated_gateway.pending_request().is_none(),
        "a gateway on an unrelated session must not see another session's pending approval"
    );

    Ok(())
}
