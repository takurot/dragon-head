use mcp_server::semantic_state_json_schema;
use serde_json::Value;

#[test]
fn test_semantic_state_schema_snapshot_diff() {
    let generated = semantic_state_json_schema();
    let expected: Value = serde_json::from_str(include_str!("fixtures/semantic_state.schema.json"))
        .expect("schema fixture must be valid json");

    assert_eq!(
        generated, expected,
        "semantic state schema changed. If intentional, update fixtures/semantic_state.schema.json"
    );
}
