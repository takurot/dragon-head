use core_runtime::audit::AuditEvent;
use serde_json::json;

#[test]
fn test_audit_event_serialization() {
    let event = AuditEvent::ToolCall {
        tool_name: "test_tool".to_string(),
        args: json!({"key": "value"}),
        timestamp: 1234567890,
    };

    let serialized = serde_json::to_string(&event).expect("Failed to serialize");

    // Validate tag matches requirement
    assert!(serialized.contains(r#""type":"TOOL_CALL""#));
    assert!(serialized.contains(r#""tool_name":"test_tool""#));
}

#[test]
fn test_state_snapshot_serialization() {
    let event = AuditEvent::StateSnapshot {
        state_hash: "abcd123".to_string(),
        page_instance_id: "page-01".to_string(),
        timestamp: 1234567890,
        payload: json!({"root": {}}),
    };

    let serialized = serde_json::to_string(&event).expect("Failed to serialize");

    assert!(serialized.contains(r#""type":"STATE_SNAPSHOT""#));
    assert!(serialized.contains(r#""state_hash":"abcd123""#));
}

#[test]
fn test_hitl_event_serialization() {
    let event = AuditEvent::HitlEvent {
        event_type: "request".to_string(),
        reason: Some("policy violation".to_string()),
        user_id: None,
        timestamp: 12345,
    };

    let serialized = serde_json::to_string(&event).unwrap();
    assert!(serialized.contains(r#""type":"HITL_EVENT""#));
    assert!(serialized.contains(r#""event_type":"request""#));
}

#[test]
fn test_policy_decision_serialization() {
    let event = AuditEvent::PolicyDecision {
        rule_id: "rule-123".to_string(),
        action: "click".to_string(),
        decision: "block".to_string(),
        destination_fingerprint: None,
        timestamp: 12345,
    };

    let serialized = serde_json::to_string(&event).unwrap();
    assert!(serialized.contains(r#""type":"POLICY_DECISION""#));
    assert!(serialized.contains(r#""decision":"block""#));
}

#[test]
fn navigation_policy_evidence_is_non_reversible_and_legacy_json_remains_compatible() {
    let fingerprint = format!("sha256:{}", "ab".repeat(32));
    let event = AuditEvent::PolicyDecision {
        rule_id: "navigation-rule".to_string(),
        action: "navigate".to_string(),
        decision: "allow".to_string(),
        destination_fingerprint: Some(fingerprint.clone()),
        timestamp: 12346,
    };
    let serialized = serde_json::to_string(&event).unwrap();
    assert!(serialized.contains(&fingerprint));
    assert!(!serialized.contains("query-secret"));
    assert!(!serialized.contains("fragment-secret"));

    let legacy = r#"{"type":"POLICY_DECISION","rule_id":"r","action":"click","decision":"allow","timestamp":1}"#;
    let decoded: AuditEvent = serde_json::from_str(legacy).unwrap();
    assert!(matches!(
        decoded,
        AuditEvent::PolicyDecision {
            destination_fingerprint: None,
            ..
        }
    ));
}

#[test]
fn test_state_patch_serialization() {
    let event = AuditEvent::StatePatch {
        state_hash: "patch-hash-456".to_string(),
        page_instance_id: "page-02".to_string(),
        timestamp: 9876543210,
        patch: serde_json::json!([{"op": "replace", "path": "/title", "value": "New"}]),
    };

    let serialized = serde_json::to_string(&event).unwrap();
    assert!(serialized.contains(r#""type":"STATE_PATCH""#));
    assert!(serialized.contains(r#""state_hash":"patch-hash-456""#));
}

#[test]
fn test_visual_capture_serialization() {
    let event = AuditEvent::VisualCapture {
        trigger: "get_visual".to_string(),
        marks_count: 42,
        timestamp: 1111111111,
    };

    let serialized = serde_json::to_string(&event).unwrap();
    assert!(serialized.contains(r#""type":"VISUAL_CAPTURE""#));
    assert!(serialized.contains(r#""trigger":"get_visual""#));
    assert!(serialized.contains(r#""marks_count":42"#));
}
