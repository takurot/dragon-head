use jsonschema::validator_for;
use mcp_server::semantic_state_json_schema;
use serde_json::Value;

#[test]
fn test_semantic_state_schema_validates_spec_sample() {
    let schema = semantic_state_json_schema();
    let validator = validator_for(&schema).expect("schema must compile");

    let sample_raw = include_str!("fixtures/semantic_state_sample.json");
    let sample: Value = serde_json::from_str(sample_raw).expect("fixture must be valid json");

    let errors: Vec<String> = validator
        .iter_errors(&sample)
        .map(|err| err.to_string())
        .collect();

    assert!(
        errors.is_empty(),
        "semantic state sample failed schema validation: {errors:?}"
    );
}

#[test]
fn test_semantic_state_schema_keeps_required_shape() {
    let schema = semantic_state_json_schema();
    let required = schema["required"]
        .as_array()
        .expect("schema required must be array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>();

    assert!(required.contains(&"metadata"));
    assert!(required.contains(&"interactive_elements"));

    let element_required = schema["properties"]["interactive_elements"]["items"]["required"]
        .as_array()
        .expect("interactive element required must be array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>();

    for field in [
        "id",
        "stable_key",
        "alias",
        "role",
        "name",
        "attributes",
        "bbox",
        "policy_flags",
    ] {
        assert!(element_required.contains(&field));
    }
}
