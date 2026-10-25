use core_runtime::audit::AuditEvent;
use core_runtime::audit_replay::replay_events;
use serde_json::{json, Value};

#[test]
fn audit_replay_matches_shared_json_patch_contract() {
    // Compare the common supported subset. json-patch rejects a root move
    // source even when source == destination; the valid root no-op remains a
    // Python-only regression in scripts/test_audit_replay.py.
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/audit_patch_contract.json")).unwrap();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let original = case["document"].clone();
        let mut document = original.clone();
        let patch: json_patch::Patch = serde_json::from_value(case["patch"].clone()).unwrap();
        let result = json_patch::patch(&mut document, &patch);
        let rejected = case["error"].as_bool().unwrap_or(false);
        assert_eq!(result.is_err(), rejected, "{name}: {result:?}");
        let mut replay_patch = case["patch"].as_array().unwrap().clone();
        if !rejected {
            assert_eq!(document, case["expected"], "{name}");
            // A root test checks the private replay snapshot without expanding
            // ReplayReport's public output contract just for these tests.
            replay_patch.push(json!({"op": "test", "path": "", "value": case["expected"]}));
        }
        let events = [
            AuditEvent::StateSnapshot {
                state_hash: "before".to_string(),
                page_instance_id: "fixture".to_string(),
                timestamp: 1,
                payload: original.clone(),
            },
            AuditEvent::StatePatch {
                state_hash: "after".to_string(),
                page_instance_id: "fixture".to_string(),
                timestamp: 2,
                patch: Value::Array(replay_patch),
            },
        ];
        let result = replay_events(&events);
        assert_eq!(result.is_err(), rejected, "{name}: {result:?}");
        if let Ok(report) = result {
            assert_eq!(report.state_chain.len(), 2, "{name}");
        }
        match &events[0] {
            AuditEvent::StateSnapshot { payload, .. } => assert_eq!(payload, &original, "{name}"),
            _ => unreachable!(),
        }
    }
}
