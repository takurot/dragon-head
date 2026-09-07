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

    // Locate the purchase button by role+name rather than a hardcoded [0]
    // index, so this doesn't silently start targeting the wrong element if
    // the fixture HTML gains another interactive element (issue #283).
    let purchase_button = state["interactive_elements"]
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
    let target_id = purchase_button["id"].as_i64().expect("target id");
    let stable_key = purchase_button["stable_key"]
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

    // Verify the click was actually blocked, not just that the response
    // reported it as pending — without this, a policy-gate bug that let
    // the action through anyway (while still returning the
    // "requires_human_approval" status) would go undetected here and only
    // surface, ambiguously, in the final post-approval assertion below
    // (issue #283).
    let clicked_before_approval = server
        .backend_mut()
        .page()
        .evaluate_script("document.body.dataset.clicked")?
        .value
        .and_then(|v| v.as_str().map(ToOwned::to_owned));
    assert_eq!(
        clicked_before_approval, None,
        "click must be blocked pending human approval, not executed early"
    );

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
