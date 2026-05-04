use std::{fs, path::PathBuf};

use core_runtime::policy::PolicyEngine;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn test_default_policy_rules_match_schema() -> anyhow::Result<()> {
    let schema_path = manifest_dir().join("policy/policy_rules.schema.json");
    let rules_path = manifest_dir().join("policy/default_rules.json");

    let schema_json: serde_json::Value = serde_json::from_str(&fs::read_to_string(&schema_path)?)?;
    let rules_json: serde_json::Value = serde_json::from_str(&fs::read_to_string(&rules_path)?)?;

    let validator = jsonschema::validator_for(&schema_json)?;
    let errors = validator
        .iter_errors(&rules_json)
        .map(|err| err.to_string())
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "default_rules.json is schema-invalid:\n{}",
        errors.join("\n")
    );

    Ok(())
}

#[test]
fn test_default_policy_rules_compile() -> anyhow::Result<()> {
    let rules_path = manifest_dir().join("policy/default_rules.json");
    let engine = PolicyEngine::try_from_file(rules_path)?;

    assert!(
        !engine.rules().is_empty(),
        "default policy rules must define at least one rule"
    );
    Ok(())
}

#[test]
fn test_examples_sample_policy_json_is_schema_valid() -> anyhow::Result<()> {
    let schema_path = manifest_dir().join("policy/policy_rules.schema.json");
    let sample_path = manifest_dir()
        .parent()
        .unwrap()
        .join("examples/sample_policy.json");

    let schema_json: serde_json::Value = serde_json::from_str(&fs::read_to_string(&schema_path)?)?;
    let sample_json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(&sample_path)
            .map_err(|e| anyhow::anyhow!("examples/sample_policy.json not found: {e}"))?,
    )?;

    let validator = jsonschema::validator_for(&schema_json)?;
    let errors = validator
        .iter_errors(&sample_json)
        .map(|err| err.to_string())
        .collect::<Vec<_>>();
    assert!(
        errors.is_empty(),
        "examples/sample_policy.json is schema-invalid:\n{}",
        errors.join("\n")
    );
    Ok(())
}

#[test]
fn test_examples_sample_policy_json_loads_in_policy_engine() -> anyhow::Result<()> {
    let sample_path = manifest_dir()
        .parent()
        .unwrap()
        .join("examples/sample_policy.json");
    let engine = PolicyEngine::try_from_file(&sample_path)
        .map_err(|e| anyhow::anyhow!("examples/sample_policy.json failed to load: {e}"))?;
    assert!(
        !engine.rules().is_empty(),
        "examples/sample_policy.json must define at least one rule"
    );
    Ok(())
}
