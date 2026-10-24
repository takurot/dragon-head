//! ISSUE-371: when an out-of-band approver (the embedded HITL bridge) is configured, the
//! in-band `ask_human` tool must not be able to grant the pending policy approval -- otherwise
//! the agent whose action was held can approve itself and bypass the reviewer.
use core_runtime::{ApprovalScope, BrowserClient, PolicyAction, PolicyRule};
use hitl_bridge::gateway::{ApprovalGateway, PageSessionGateway};
use mcp_server::{CoreRuntimeBackend, McpServer};
use serde_json::{json, Value};

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

fn locate_purchase_button(state: &Value) -> (i64, String) {
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
    (
        element["id"].as_i64().expect("target id"),
        element["stable_key"]
            .as_str()
            .expect("stable key")
            .to_string(),
    )
}

fn click(server: &mut McpServer<CoreRuntimeBackend>, id: i64, key: &str) -> anyhow::Result<Value> {
    server.call_tool(
        "act",
        json!({ "target_id": id, "target_stable_key": key, "action": "click" }),
    )
}

fn clicked(server: &mut McpServer<CoreRuntimeBackend>) -> anyhow::Result<Option<String>> {
    Ok(server
        .backend_mut()
        .page()
        .evaluate_script("document.body.dataset.clicked")?
        .value
        .and_then(|v| v.as_str().map(ToOwned::to_owned)))
}

#[test]
fn ask_human_cannot_grant_when_in_band_approval_is_disabled() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    setup_gated_button(&page)?;

    let mut backend = CoreRuntimeBackend::new_with_client(client, page);
    backend.set_in_band_approval(false);
    let gateway = PageSessionGateway::with_provider(backend.page_provider());
    let mut server = McpServer::new(backend);

    let state = server.call_tool(
        "get_state",
        json!({ "format": "json", "force_refresh": true }),
    )?;
    let (target_id, stable_key) = locate_purchase_button(&state);

    let first = click(&mut server, target_id, &stable_key)?;
    assert_eq!(first["status"], json!("requires_human_approval"));

    // The agent tries to approve itself.
    let attempt = server.call_tool(
        "ask_human",
        json!({ "reason": "agent self-approve", "context": true }),
    )?;
    assert_eq!(attempt["approved"], json!(false));
    assert_eq!(attempt["pending"], json!(true));
    assert!(
        attempt.get("context").is_none(),
        "a denied self-approval must not echo the pending request context"
    );

    let second = click(&mut server, target_id, &stable_key)?;
    assert_eq!(
        second["status"],
        json!("requires_human_approval"),
        "ask_human must not turn the pending request into an executable approval"
    );
    assert_eq!(clicked(&mut server)?, None);

    // The out-of-band approver (what the bridge's signed callback ends up calling) still can.
    let pending = gateway
        .pending_request()
        .expect("request must still be pending for the out-of-band approver");
    gateway.approve(pending.id)?;

    let third = click(&mut server, target_id, &stable_key)?;
    assert_eq!(third["status"], json!("ok"));
    assert_eq!(clicked(&mut server)?.as_deref(), Some("yes"));

    Ok(())
}
