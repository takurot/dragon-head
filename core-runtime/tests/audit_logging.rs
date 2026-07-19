use std::time::{Duration, Instant};

use anyhow::Context;

use core_runtime::{
    audit::AuditEvent,
    audit_replay::{replay_events, ReplayError},
    sre::{normalize_dom, LoadProfile, SemanticState},
    BrowserClient,
};

#[test]
fn test_audit_logging_sequence_and_pii_masking() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <input id="email" type="email" value="seed@example.com" />
                <input id="password" type="password" value="MySecretP@ssw0rd!" />
                <div id="cc-note">Card: 4000 1234 5678 9012 345</div>
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
        Some("alice@example.com 4000 1234 5678 9012 345"),
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
        !snapshot_text.contains("MySecretP@ssw0rd!"),
        "state snapshot must mask password value"
    );
    assert!(
        !snapshot_text.contains("4000 1234 5678 9012 345"),
        "state snapshot must mask card number"
    );
    let email_input =
        find_node_by_dom_id(snapshot_payload, "email").context("email input snapshot missing")?;
    assert_eq!(
        email_input
            .pointer("/attributes/value")
            .and_then(|value| value.as_str()),
        Some("***"),
        "email input value should be masked in state snapshot"
    );

    let password_input = find_node_by_dom_id(snapshot_payload, "password")
        .context("password input snapshot missing")?;
    assert_eq!(
        password_input
            .pointer("/attributes/value")
            .and_then(|value| value.as_str()),
        Some("***"),
        "password input value should be masked in state snapshot"
    );

    let card_text =
        find_node_by_role_and_label_fragment(snapshot_payload, "text", "****-****-****-XXXX")
            .context("redacted card text snapshot missing")?;
    assert_eq!(
        card_text.get("label").and_then(|value| value.as_str()),
        Some("Card: ****-****-****-XXXX"),
        "card text should preserve the redaction marker"
    );

    Ok(())
}

#[test]
fn test_audit_logging_verify_text_masks_expected_text() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <div id="status">Ready for verification</div>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let root = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Interactive, &root)?;
    let state = SemanticState::new(sem, LoadProfile::Interactive);
    let (target_id, _) =
        find_node_info_by_dom_id(state.root(), "status").context("status element not found")?;

    page.clear_audit_events();
    let result = page.verify_text(
        target_id,
        None,
        "MySecretP@ssw0rd! alice@example.com 4000 1234 5678 9012 345",
    );
    assert!(
        result.is_err(),
        "verify_text should fail on mismatched expectation"
    );

    let events = wait_for_events(&page, 1, Duration::from_secs(2));
    let tool_args = events
        .iter()
        .find_map(|event| match event {
            AuditEvent::ToolCall {
                tool_name, args, ..
            } if tool_name == "verify_text" => Some(args),
            _ => None,
        })
        .context("TOOL_CALL(verify_text) event not found")?;

    let tool_args_text = serde_json::to_string(tool_args)?;
    assert!(
        !tool_args_text.contains("MySecretP@ssw0rd!"),
        "verify_text tool args should redact password-like secrets"
    );
    assert!(
        !tool_args_text.contains("alice@example.com"),
        "verify_text tool args should redact email addresses"
    );
    assert!(
        !tool_args_text.contains("4000 1234 5678 9012 345"),
        "verify_text tool args should redact card numbers"
    );
    assert_eq!(
        tool_args
            .get("expected_text")
            .and_then(|value| value.as_str()),
        Some("***"),
        "verify_text expected_text should be masked"
    );

    Ok(())
}

#[test]
fn test_repeated_state_capture_emits_state_patch_for_incremental_update() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <ul id="catalog">
                    <li>Product 00</li>
                    <li>Product 01</li>
                    <li>Product 02</li>
                    <li>Product 03</li>
                    <li>Product 04</li>
                    <li>Product 05</li>
                    <li>Product 06</li>
                    <li>Product 07</li>
                    <li>Product 08</li>
                    <li>Product 09</li>
                    <li>Product 10</li>
                    <li>Product 11</li>
                    <li>Product 12</li>
                    <li>Product 13</li>
                    <li>Product 14</li>
                    <li>Product 15</li>
                    <li>Product 16</li>
                    <li>Product 17</li>
                    <li>Product 18</li>
                    <li>Product 19</li>
                    <li>Product 20</li>
                    <li>Product 21</li>
                    <li>Product 22</li>
                    <li>Product 23</li>
                    <li>Product 24</li>
                    <li>Product 25</li>
                    <li>Product 26</li>
                    <li>Product 27</li>
                    <li>Product 28</li>
                    <li>Product 29</li>
                    <li>Product 30</li>
                    <li>Product 31</li>
                    <li>Product 32</li>
                    <li>Product 33</li>
                    <li>Product 34</li>
                    <li>Product 35</li>
                    <li>Product 36</li>
                    <li>Product 37</li>
                    <li>Product 38</li>
                    <li>Product 39</li>
                </ul>
                <script>
                    window.renameProduct = () => {
                        const item = document.querySelectorAll('#catalog li')[22];
                        item.innerText = 'Product 22 updated';
                    };
                </script>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    page.clear_audit_events();
    let baseline = page.capture_semantic_state(LoadProfile::Minimal)?;
    page.evaluate_script("window.renameProduct()")?;
    let updated = page.capture_semantic_state(LoadProfile::Minimal)?;

    assert_ne!(baseline.state_hash(), updated.state_hash());

    let events = wait_for_events(&page, 2, Duration::from_secs(2));
    assert!(
        matches!(events.first(), Some(AuditEvent::StateSnapshot { .. })),
        "first capture should emit a full state snapshot"
    );

    let patch = events.iter().find_map(|event| {
        if let AuditEvent::StatePatch { patch, .. } = event {
            Some(patch)
        } else {
            None
        }
    });
    let patch = patch.context("STATE_PATCH event not found for incremental capture")?;
    assert!(
        patch
            .as_array()
            .is_some_and(|operations| !operations.is_empty()),
        "state patch must contain at least one RFC 6902 operation"
    );

    Ok(())
}

/// Replay tests — no browser required.

#[test]
fn replay_fixture_produces_report_with_all_event_types() -> anyhow::Result<()> {
    let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/audit_replay_fixture.ndjson");
    let content = std::fs::read_to_string(&fixture_path)
        .with_context(|| format!("failed to read fixture: {}", fixture_path.display()))?;

    let events: Vec<AuditEvent> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()
        .context("failed to parse fixture NDJSON")?;

    let report = replay_events(&events)?;

    assert_eq!(report.total_events, 5, "should count all 5 fixture events");

    // State chain: one snapshot + one patch
    assert_eq!(report.state_chain.len(), 2);
    assert_eq!(report.state_chain[0].state_hash, "sha256:a1b2c3d4");
    assert_eq!(report.state_chain[0].timestamp, 1000);
    assert_eq!(report.state_chain[1].state_hash, "sha256:b2c3d4e5");

    // Tool calls
    assert_eq!(report.tool_calls.len(), 1);
    assert_eq!(report.tool_calls[0].tool_name, "act");
    assert!(
        report.tool_calls[0].has_redacted_args,
        "fixture tool call contains *** args"
    );

    // Policy decisions
    assert_eq!(report.policy_decisions.len(), 1);
    assert_eq!(report.policy_decisions[0].rule_id, "PII-001");
    assert_eq!(report.policy_decisions[0].decision, "allow");

    // HITL events
    assert_eq!(report.hitl_events.len(), 1);
    assert_eq!(report.hitl_events[0].event_type, "approval_requested");

    assert!(report.has_redacted_content, "fixture contains *** values");
    Ok(())
}

#[test]
fn replay_report_includes_timestamps_state_hashes_and_decisions() -> anyhow::Result<()> {
    let events = vec![
        AuditEvent::StateSnapshot {
            state_hash: "sha256:aabbcc".to_string(),
            page_instance_id: "p-01".to_string(),
            timestamp: 1_000,
            payload: serde_json::json!({"role": "document", "label": "Test"}),
        },
        AuditEvent::PolicyDecision {
            rule_id: "R-42".to_string(),
            action: "block".to_string(),
            decision: "deny".to_string(),
            destination_fingerprint: None,
            timestamp: 2_000,
        },
        AuditEvent::StatePatch {
            state_hash: "sha256:bbccdd".to_string(),
            page_instance_id: "p-01".to_string(),
            timestamp: 3_000,
            patch: serde_json::json!([{"op": "replace", "path": "/label", "value": "Updated"}]),
        },
    ];

    let report = replay_events(&events)?;

    assert_eq!(report.state_chain[0].timestamp, 1_000);
    assert_eq!(report.state_chain[0].state_hash, "sha256:aabbcc");
    assert_eq!(report.state_chain[1].timestamp, 3_000);
    assert_eq!(report.state_chain[1].state_hash, "sha256:bbccdd");

    assert_eq!(report.policy_decisions[0].rule_id, "R-42");
    assert_eq!(report.policy_decisions[0].decision, "deny");
    assert_eq!(report.policy_decisions[0].timestamp, 2_000);
    Ok(())
}

#[test]
fn replay_report_detects_redacted_content_in_tool_args() {
    let events = vec![AuditEvent::ToolCall {
        tool_name: "act".to_string(),
        args: serde_json::json!({"action": "type", "value": "***"}),
        timestamp: 100,
    }];

    let report = replay_events(&events).expect("replay must succeed");
    assert!(report.has_redacted_content, "*** in args should set flag");
    assert!(report.tool_calls[0].has_redacted_args);
}

#[test]
fn replay_patch_without_prior_snapshot_is_an_error() {
    let events = vec![AuditEvent::StatePatch {
        state_hash: "sha256:orphan".to_string(),
        page_instance_id: "p-01".to_string(),
        timestamp: 1_000,
        patch: serde_json::json!([{"op": "replace", "path": "/label", "value": "Orphan"}]),
    }];

    let result = replay_events(&events);
    assert!(
        matches!(result, Err(ReplayError::NoBaseSnapshot { .. })),
        "orphan patch must produce NoBaseSnapshot error"
    );
}

#[test]
fn replay_invalid_patch_chain_produces_actionable_error() {
    let events = vec![
        AuditEvent::StateSnapshot {
            state_hash: "sha256:base".to_string(),
            page_instance_id: "p-01".to_string(),
            timestamp: 1_000,
            payload: serde_json::json!({"role": "document"}),
        },
        AuditEvent::StatePatch {
            state_hash: "sha256:bad".to_string(),
            page_instance_id: "p-01".to_string(),
            timestamp: 2_000,
            // "test" op on non-existent path will fail
            patch: serde_json::json!([{"op": "test", "path": "/nonexistent", "value": "x"}]),
        },
    ];

    let result = replay_events(&events);
    assert!(
        matches!(result, Err(ReplayError::PatchFailed { .. })),
        "invalid patch should produce PatchFailed error"
    );
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

fn find_node_by_dom_id<'a>(
    node: &'a serde_json::Value,
    dom_id: &str,
) -> Option<&'a serde_json::Value> {
    let is_match = node
        .get("attributes")
        .and_then(|attributes| attributes.as_object())
        .and_then(|attributes| attributes.get("id"))
        .and_then(|value| value.as_str())
        == Some(dom_id);
    if is_match {
        return Some(node);
    }

    node.get("children")?
        .as_array()?
        .iter()
        .find_map(|child| find_node_by_dom_id(child, dom_id))
}

fn find_node_by_role_and_label_fragment<'a>(
    node: &'a serde_json::Value,
    role: &str,
    label_fragment: &str,
) -> Option<&'a serde_json::Value> {
    let role_matches = node.get("role").and_then(|value| value.as_str()) == Some(role);
    let label_matches = node
        .get("label")
        .and_then(|value| value.as_str())
        .is_some_and(|label| label.contains(label_fragment));
    if role_matches && label_matches {
        return Some(node);
    }

    node.get("children")?
        .as_array()?
        .iter()
        .find_map(|child| find_node_by_role_and_label_fragment(child, role, label_fragment))
}
