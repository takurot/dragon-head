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
        timestamp: 12345,
    };

    let serialized = serde_json::to_string(&event).unwrap();
    assert!(serialized.contains(r#""type":"POLICY_DECISION""#));
    assert!(serialized.contains(r#""decision":"block""#));
}
