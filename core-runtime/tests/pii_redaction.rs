/// Integration tests verifying that PII never leaks through the SRE pipeline
/// or the Audit Sink (Exit Criteria for PR-27 / ISSUE-17).
use core_runtime::{
    audit::{AuditEvent, AuditLogger},
    privacy::{DomainPattern, PiiRedactor},
    sre::{AsyncPipeline, AsyncPipelineConfig, LoadProfile, SemanticNode, SemanticState},
};
use serde_json::json;
use std::{collections::BTreeMap, time::Duration};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn node_with_label(role: &str, label: &str) -> SemanticNode {
    SemanticNode {
        role: role.to_string(),
        label: Some(label.to_string()),
        children: vec![],
        attributes: None,
        stable_key: None,
        ambiguous: false,
        alias: None,
        backend_node_id: 0,
        security_flags: vec![],
    }
}

fn node_with_attrs(role: &str, attrs: BTreeMap<String, String>) -> SemanticNode {
    SemanticNode {
        role: role.to_string(),
        label: None,
        children: vec![],
        attributes: Some(attrs),
        stable_key: None,
        ambiguous: false,
        alias: None,
        backend_node_id: 0,
        security_flags: vec![],
    }
}

fn simple_state(root: SemanticNode) -> SemanticState {
    SemanticState::new(root, LoadProfile::default())
}

// ---------------------------------------------------------------------------
// PiiRedactor unit-level integration
// ---------------------------------------------------------------------------

#[test]
fn pii_redactor_masks_email_and_cc_in_text() {
    let r = PiiRedactor::new();
    let out = r.redact_text("Pay 4111-1111-1111-1111 or email alice@example.com");
    assert!(!out.contains("4111"), "credit card must be masked");
    assert!(!out.contains("alice@example.com"), "email must be masked");
    assert!(out.contains("****-****-****-XXXX"));
    assert!(out.contains("***"));
}

#[test]
fn pii_redactor_domain_pattern_masks_ssn() {
    // US SSN format (9 digits with dashes) does not match the CC regex (13-19 digits),
    // so it should be caught exclusively by the domain pattern.
    let dp = DomainPattern::new("ssn", r"\b\d{3}-\d{2}-\d{4}\b", "[SSN]").unwrap();
    let r = PiiRedactor::with_domain_patterns(vec![dp]);
    let out = r.redact_text("SSN: 123-45-6789 on file");
    assert!(!out.contains("123-45-6789"), "SSN must be masked");
    assert!(out.contains("[SSN]"));
}

#[test]
fn pii_redactor_json_masks_password_field() {
    let r = PiiRedactor::new();
    let input = json!({ "username": "alice", "password": "secret123" });
    let out = r.redact_json(&input);
    assert_eq!(out["username"], "alice");
    assert_eq!(out["password"], "***");
}

#[test]
fn pii_redactor_json_tool_args_masks_value_field() {
    let r = PiiRedactor::new();
    let input = json!({ "selector": "#email-input", "value": "bob@example.com" });
    let out = r.redact_json_tool_args(&input);
    assert_eq!(out["selector"], "#email-input");
    assert_eq!(out["value"], "***");
}

// ---------------------------------------------------------------------------
// SRE pipeline exit hook
// ---------------------------------------------------------------------------

#[test]
fn sre_pipeline_fast_state_has_email_masked() {
    let cfg = AsyncPipelineConfig {
        render_queue_capacity: 4,
        sre_queue_capacity: 4,
        audit_queue_capacity: 4,
        ..Default::default()
    };
    let pipeline = AsyncPipeline::new(cfg);

    let root = SemanticNode {
        role: "root".to_string(),
        label: None,
        children: vec![node_with_label("input", "alice@example.com")],
        attributes: None,
        stable_key: None,
        ambiguous: false,
        alias: None,
        backend_node_id: 0,
        security_flags: vec![],
    };

    let handle = pipeline
        .submit_state(simple_state(root))
        .expect("submit should succeed");

    let fast = handle
        .recv_fast(Duration::from_millis(500))
        .expect("should receive fast state");

    for elem in &fast.interactive_elements {
        if let Some(label) = &elem.label {
            assert!(
                !label.contains("alice@example.com"),
                "email must not appear in SRE fast state label: {label}"
            );
        }
    }
}

#[test]
fn sre_pipeline_fast_state_masks_cc_in_node_label() {
    let cfg = AsyncPipelineConfig::default();
    let pipeline = AsyncPipeline::new(cfg);

    let root = SemanticNode {
        role: "root".to_string(),
        label: None,
        children: vec![node_with_label("button", "Pay 4111-1111-1111-1111 now")],
        attributes: None,
        stable_key: None,
        ambiguous: false,
        alias: None,
        backend_node_id: 0,
        security_flags: vec![],
    };

    let handle = pipeline
        .submit_state(simple_state(root))
        .expect("submit should succeed");

    let fast = handle
        .recv_fast(Duration::from_millis(500))
        .expect("should receive fast state");

    for elem in &fast.interactive_elements {
        if let Some(label) = &elem.label {
            assert!(
                !label.contains("4111"),
                "credit card must not appear in SRE fast state: {label}"
            );
        }
    }
}

#[test]
fn sre_pipeline_full_state_masks_sensitive_attribute_values() {
    let cfg = AsyncPipelineConfig::default();
    let pipeline = AsyncPipeline::new(cfg);

    let mut attrs = BTreeMap::new();
    attrs.insert("type".to_string(), "password".to_string());
    attrs.insert("value".to_string(), "my-secret-password".to_string());

    let root = SemanticNode {
        role: "form".to_string(),
        label: None,
        children: vec![node_with_attrs("input", attrs)],
        attributes: None,
        stable_key: None,
        ambiguous: false,
        alias: None,
        backend_node_id: 0,
        security_flags: vec![],
    };

    let handle = pipeline
        .submit_state(simple_state(root))
        .expect("submit should succeed");

    let full = handle
        .recv_full(Duration::from_millis(500))
        .expect("should receive full state");

    // The full state collects form nodes and their children (projected flat).
    // The input child node should have its `value` attribute masked because
    // the sibling attribute `type="password"` marks it as sensitive.
    let input_node = full
        .forms
        .iter()
        .find(|n| n.role == "input")
        .expect("input node must be in full.forms");

    let val = input_node
        .attributes
        .as_ref()
        .and_then(|a| a.get("value"))
        .map(String::as_str)
        .unwrap_or("");

    assert!(
        !val.contains("my-secret-password"),
        "password value must be masked in SRE full state, got: {val}"
    );
}

// ---------------------------------------------------------------------------
// Audit Sink entry hook (via AuditLogger)
// ---------------------------------------------------------------------------

#[test]
fn audit_logger_tool_call_redacts_email_in_value_field() {
    let logger = AuditLogger::new();
    logger.clear_recent_events();

    logger.log(AuditEvent::ToolCall {
        tool_name: "fill".to_string(),
        args: json!({ "selector": "#email", "value": "alice@example.com" }),
        timestamp: 0,
    });

    let events = logger.recent_events();
    let AuditEvent::ToolCall { args, .. } = &events[0] else {
        panic!("expected ToolCall");
    };
    assert_eq!(
        args["value"], "***",
        "email in value field must be redacted in audit log"
    );
}

#[test]
fn audit_logger_tool_call_redacts_password_key() {
    let logger = AuditLogger::new();
    logger.clear_recent_events();

    logger.log(AuditEvent::ToolCall {
        tool_name: "login".to_string(),
        args: json!({ "username": "alice", "password": "hunter2" }),
        timestamp: 0,
    });

    let events = logger.recent_events();
    let AuditEvent::ToolCall { args, .. } = &events[0] else {
        panic!("expected ToolCall");
    };
    assert_eq!(args["password"], "***");
    assert_eq!(args["username"], "alice");
}

#[test]
fn audit_logger_state_snapshot_redacts_email_in_payload() {
    let logger = AuditLogger::new();
    logger.clear_recent_events();

    logger.log(AuditEvent::StateSnapshot {
        state_hash: "abc".to_string(),
        page_instance_id: "page-1".to_string(),
        timestamp: 0,
        payload: json!({ "label": "alice@example.com" }),
    });

    let events = logger.recent_events();
    let AuditEvent::StateSnapshot { payload, .. } = &events[0] else {
        panic!("expected StateSnapshot");
    };
    assert!(
        !payload["label"]
            .as_str()
            .unwrap_or("")
            .contains("alice@example.com"),
        "email must be redacted in StateSnapshot payload"
    );
}

#[test]
fn audit_logger_credit_card_redacted_in_tool_call_args() {
    let logger = AuditLogger::new();
    logger.clear_recent_events();

    logger.log(AuditEvent::ToolCall {
        tool_name: "fill".to_string(),
        args: json!({ "note": "Card 4111-1111-1111-1111" }),
        timestamp: 0,
    });

    let events = logger.recent_events();
    let AuditEvent::ToolCall { args, .. } = &events[0] else {
        panic!("expected ToolCall");
    };
    let note = args["note"].as_str().unwrap_or("");
    assert!(
        !note.contains("4111-1111-1111-1111"),
        "credit card must be redacted: {note}"
    );
}
