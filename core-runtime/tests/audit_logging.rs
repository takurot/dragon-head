use std::time::{Duration, Instant};

use anyhow::Context;
use core_runtime::{
    audit::AuditEvent,
    sre::{normalize_dom, LoadProfile, SemanticState},
    BrowserClient,
};

fn should_skip() -> bool {
    std::env::var("CI").is_ok() && std::env::var("CHROME_INSTALLED").is_err()
}

#[test]
fn test_audit_logging_sequence_and_pii_masking() -> anyhow::Result<()> {
    if should_skip() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <input id="email" type="email" value="seed@example.com" />
                <div id="cc-note">Card: 5555-4444-3333-2222</div>
                <button id="submit">Submit</button>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let root = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Interactive, &root)?;
    let state = SemanticState::new(sem, LoadProfile::Interactive);
    let (input_id, input_key) =
        find_node_info_by_dom_id(state.root(), "email").context("email input not found")?;

    page.clear_audit_events();
    page.act(
        Some(input_id),
        Some(&input_key),
        "type",
        Some("alice@example.com 4111-1111-1111-1111"),
    )?;

    let events = wait_for_events(&page, 3, Duration::from_secs(2));
    assert!(
        !events.is_empty(),
        "audit events should be emitted for act execution"
    );

    let tool_call_idx = events
        .iter()
        .position(
            |event| matches!(event, AuditEvent::ToolCall { tool_name, .. } if tool_name == "act"),
        )
        .context("TOOL_CALL(act) event not found")?;
    let snapshot_idx = events
        .iter()
        .position(|event| matches!(event, AuditEvent::StateSnapshot { .. }))
        .context("STATE_SNAPSHOT event not found")?;
    assert!(
        tool_call_idx < snapshot_idx,
        "TOOL_CALL should be recorded before STATE_SNAPSHOT for act path"
    );

    let tool_args = match &events[tool_call_idx] {
        AuditEvent::ToolCall { args, .. } => args,
        _ => unreachable!(),
    };
    let tool_args_text = serde_json::to_string(tool_args)?;
    assert!(
        !tool_args_text.contains("alice@example.com"),
        "tool args should redact email addresses"
    );
    assert!(
        !tool_args_text.contains("4111-1111-1111-1111"),
        "tool args should redact raw card numbers"
    );
    assert_eq!(
        tool_args.get("value").and_then(|v| v.as_str()),
        Some("***"),
        "tool args value should be masked"
    );

    let snapshot_payload = events.iter().find_map(|event| {
        if let AuditEvent::StateSnapshot { payload, .. } = event {
            Some(payload)
        } else {
            None
        }
    });
    let snapshot_payload = snapshot_payload.context("state snapshot payload missing")?;
    let snapshot_text = serde_json::to_string(snapshot_payload)?;
    assert!(
        !snapshot_text.contains("seed@example.com"),
        "state snapshot must mask email value"
    );
    assert!(
        !snapshot_text.contains("5555-4444-3333-2222"),
        "state snapshot must mask card number"
    );
    assert!(
        snapshot_text.contains("****-****-****-XXXX"),
        "state snapshot should keep redaction marker for card numbers"
    );

    Ok(())
}

fn wait_for_events(
    page: &core_runtime::PageSession,
    min_events: usize,
    timeout: Duration,
) -> Vec<AuditEvent> {
    let start = Instant::now();
    loop {
        let events = page.audit_events();
        if events.len() >= min_events || start.elapsed() >= timeout {
            return events;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn find_node_info_by_dom_id(
    node: &core_runtime::sre::state::SemanticNode,
    dom_id: &str,
) -> Option<(i64, String)> {
    if node
        .attributes
        .as_ref()
        .and_then(|attrs| attrs.get("id"))
        .is_some_and(|id| id == dom_id)
    {
        return Some((node.backend_node_id, node.stable_key.clone()?));
    }

    for child in &node.children {
        if let Some(found) = find_node_info_by_dom_id(child, dom_id) {
            return Some(found);
        }
    }

    None
}
