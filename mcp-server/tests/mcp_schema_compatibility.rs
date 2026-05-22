use jsonschema::validator_for;
use mcp_server::semantic_state_json_schema;
use serde_json::{json, Value};

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

    // security_flags must NOT be required — omitting it is valid for backward compat.
    assert!(
        !element_required.contains(&"security_flags"),
        "security_flags must be optional (backward compat)"
    );
}

/// security_flags property in the schema must have the correct shape:
/// array of strings, with a descriptive description.
#[test]
fn test_schema_security_flags_property_shape() {
    let schema = semantic_state_json_schema();
    let props = &schema["properties"]["interactive_elements"]["items"]["properties"];

    let sf = &props["security_flags"];
    assert_eq!(
        sf["type"],
        json!("array"),
        "security_flags must be array type"
    );
    assert_eq!(
        sf["items"]["type"],
        json!("string"),
        "security_flags items must be strings"
    );
    assert!(
        sf["description"].is_string(),
        "security_flags must have a description"
    );
}

/// A document with security_flags on an element must validate against the schema.
#[test]
fn test_schema_accepts_element_with_security_flags() {
    let schema = semantic_state_json_schema();
    let validator = validator_for(&schema).expect("schema must compile");

    let doc = json!({
        "metadata": {
            "url": "https://example.com",
            "page_instance_id": "pid-1",
            "state_hash": "abc123",
            "load_profile": "interactive",
            "timestamp": 1_000_000u64
        },
        "interactive_elements": [{
            "id": 1,
            "stable_key": "key-btn-1",
            "alias": "btn_1",
            "role": "button",
            "name": "Click me",
            "attributes": {},
            "bbox": [0.0, 0.0, 100.0, 50.0],
            "policy_flags": [],
            "security_flags": ["possible_prompt_injection"]
        }]
    });

    let errors: Vec<String> = validator.iter_errors(&doc).map(|e| e.to_string()).collect();

    assert!(
        errors.is_empty(),
        "schema must accept element with security_flags: {errors:?}"
    );
}

/// A document with no security_flags field (legacy client) must also validate.
#[test]
fn test_schema_accepts_element_without_security_flags() {
    let schema = semantic_state_json_schema();
    let validator = validator_for(&schema).expect("schema must compile");

    let doc = json!({
        "metadata": {
            "url": "https://example.com",
            "page_instance_id": "pid-1",
            "state_hash": "abc123",
            "load_profile": "interactive",
            "timestamp": 1_000_000u64
        },
        "interactive_elements": [{
            "id": 1,
            "stable_key": "key-btn-1",
            "alias": "btn_1",
            "role": "button",
            "name": "Click me",
            "attributes": {},
            "bbox": [0.0, 0.0, 100.0, 50.0],
            "policy_flags": []
        }]
    });

    let errors: Vec<String> = validator.iter_errors(&doc).map(|e| e.to_string()).collect();

    assert!(
        errors.is_empty(),
        "schema must accept element without security_flags (legacy compat): {errors:?}"
    );
}
