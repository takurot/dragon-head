//! ISSUE-372: the runtime audit trail must record who/what granted a held action, not only the
//! request and rejection, and `ask_human` calls must appear as `TOOL_CALL` events.
use core_runtime::audit::AuditEvent;
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
    let html = r#"<html><body><button id="purchase">Purchase</button></body></html>"#;
    page.navigate(&format!("data:text/html,{}", urlencoding::encode(html)))?;
    Ok(())
}

fn locate_purchase_button(state: &Value) -> (i64, String) {
    let element = state["interactive_elements"]
        .as_array()
        .expect("interactive_elements array")
        .iter()
        .find(|e| {
            e["role"] == json!("button")
                && e["name"]
                    .as_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case("purchase"))
        })
        .expect("purchase button not found");
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

/// HITL and tool-call events of interest, in order, as `HITL:<type>` / `TOOL:<name>` labels
/// paired with the HITL reason (empty for tool calls).
fn trail(server: &mut McpServer<CoreRuntimeBackend>) -> Vec<(String, String)> {
    server
        .backend_mut()
        .page()
        .audit_events()
        .into_iter()
        .filter_map(|event| match event {
            AuditEvent::HitlEvent {
                event_type, reason, ..
            } => Some((format!("HITL:{event_type}"), reason.unwrap_or_default())),
            AuditEvent::ToolCall {
                tool_name, args, ..
            } if tool_name == "act" || tool_name == "ask_human" => {
                Some((format!("TOOL:{tool_name}"), args.to_string()))
            }
            _ => None,
        })
        .collect()
}

fn labels(trail: &[(String, String)]) -> Vec<&str> {
    trail.iter().map(|(label, _)| label.as_str()).collect()
}

fn gated_server() -> anyhow::Result<(McpServer<CoreRuntimeBackend>, i64, String)> {
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    setup_gated_button(&page)?;
    let mut server = McpServer::new(CoreRuntimeBackend::new_with_client(client, page));
    let state = server.call_tool(
        "get_state",
        json!({ "format": "json", "force_refresh": true }),
    )?;
    let (id, key) = locate_purchase_button(&state);
    Ok((server, id, key))
}

#[test]
fn ask_human_approval_is_recorded_between_request_and_executed_act() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let (mut server, id, key) = gated_server()?;

    assert_eq!(
        click(&mut server, id, &key)?["status"],
        json!("requires_human_approval")
    );
    server.call_tool(
        "ask_human",
        json!({ "reason": "checkout approved by me, reach jane.doe@example.com" }),
    )?;
    assert_eq!(click(&mut server, id, &key)?["status"], json!("ok"));

    let trail = trail(&mut server);
    let labels = labels(&trail);
    let position = |label: &str, from: usize| {
        labels[from..]
            .iter()
            .position(|l| *l == label)
            .map(|p| p + from)
            .unwrap_or_else(|| panic!("{label} missing after index {from} in {labels:?}"))
    };
    let request = position("HITL:request", 0);
    let ask = position("TOOL:ask_human", request);
    let approved = position("HITL:approved", ask);
    position("TOOL:act", approved);

    let reason = &trail[approved].1;
    assert!(
        reason.contains("checkout-approval"),
        "rule id missing: {reason}"
    );
    assert!(reason.contains("ask_human"), "channel missing: {reason}");
    assert!(
        trail[ask].1.contains("checkout approved by me"),
        "ask_human reason missing: {}",
        trail[ask].1
    );
    assert!(
        !trail[ask].1.contains("jane.doe@example.com"),
        "PII must be redacted in the ask_human TOOL_CALL: {}",
        trail[ask].1
    );
    Ok(())
}

#[test]
fn bridge_approval_records_its_channel_and_rejection_stays_recorded() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let (mut server, id, key) = gated_server()?;
    let gateway = PageSessionGateway::with_provider(server.backend_mut().page_provider());

    click(&mut server, id, &key)?;
    let pending = gateway.pending_request().expect("pending request");
    gateway.approve(pending.id)?;

    let after_approve = trail(&mut server);
    let approved = after_approve
        .iter()
        .find(|(label, _)| label == "HITL:approved")
        .expect("bridge approval must be audited");
    assert!(
        approved.1.contains("hitl_bridge"),
        "channel missing: {}",
        approved.1
    );

    // A fresh request that gets rejected: request -> rejected, no new approval.
    assert_eq!(click(&mut server, id, &key)?["status"], json!("ok"));
    server.backend_mut().page().clear_audit_events();
    click(&mut server, id, &key)?;
    let pending = gateway.pending_request().expect("pending request");
    gateway.reject(pending.id)?;
    let labels_after_reject: Vec<String> = trail(&mut server).into_iter().map(|t| t.0).collect();
    assert!(
        labels_after_reject
            .windows(2)
            .any(|w| w == ["HITL:request", "HITL:rejected"]),
        "{labels_after_reject:?}"
    );
    assert!(!labels_after_reject.contains(&"HITL:approved".to_string()));
    Ok(())
}
