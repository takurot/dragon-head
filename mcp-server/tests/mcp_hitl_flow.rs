use core_runtime::{ApprovalScope, BrowserClient, PolicyAction, PolicyRule};
use mcp_server::{CoreRuntimeBackend, McpServer};
use serde_json::json;

#[test]
fn test_ask_human_hitl_flow_with_policy_gate() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

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

    let mut server = McpServer::new(CoreRuntimeBackend::new(page));

    let state = server.call_tool(
        "get_state",
        json!({
            "format": "json",
            "force_refresh": true
        }),
    )?;

    let target_id = state["interactive_elements"][0]["id"]
        .as_i64()
        .expect("target id");
    let stable_key = state["interactive_elements"][0]["stable_key"]
        .as_str()
        .expect("stable key")
        .to_string();

    let first_act = server.call_tool(
        "act",
        json!({
            "target_id": target_id,
            "target_stable_key": stable_key,
            "action": "click"
        }),
    )?;

    assert_eq!(first_act["status"], json!("requires_human_approval"));

    let approval = server.call_tool(
        "ask_human",
        json!({
            "reason": "checkout requires approval",
            "context": true
        }),
    )?;

    assert_eq!(approval["approved"], json!(true));

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
