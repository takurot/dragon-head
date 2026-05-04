use anyhow::{Context, Result, ensure};
use skills_engine::{
    SkillDefinition, skill_definition_schema, validate_skill_definition, validate_skill_json,
};
use std::{fs, path::PathBuf};

const SCHEMA_FIXTURE: &str = "tests/fixtures/skill/skill_definition.schema.json";
const UPDATE_ENV: &str = "UPDATE_SKILL_SCHEMA";

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(SCHEMA_FIXTURE)
}

#[test]
fn test_skill_schema_backward_compatibility_fixture() -> Result<()> {
    let schema = skill_definition_schema();
    let path = fixture_path();

    if std::env::var(UPDATE_ENV).as_deref() == Ok("1") {
        ensure!(
            std::env::var("CI").is_err(),
            "{UPDATE_ENV}=1 is not allowed in CI. Update schema fixture locally and commit it."
        );
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create schema fixture dir: {}", parent.display())
            })?;
        }
        let text = serde_json::to_string_pretty(&schema)?;
        fs::write(&path, text)
            .with_context(|| format!("failed to write schema fixture: {}", path.display()))?;
        return Ok(());
    }

    let expected_text = fs::read_to_string(&path).with_context(|| {
        format!(
            "schema fixture missing. run `{UPDATE_ENV}=1 cargo test -p skills-engine --test skill_schema_compatibility` at {}",
            path.display()
        )
    })?;
    let expected: serde_json::Value = serde_json::from_str(&expected_text)
        .with_context(|| format!("invalid schema fixture json: {}", path.display()))?;

    assert_eq!(
        schema, expected,
        "Skill schema changed. If intentional, update fixture with `{UPDATE_ENV}=1 cargo test -p skills-engine --test skill_schema_compatibility`."
    );
    Ok(())
}

#[test]
fn test_skill_schema_validates_examples() {
    let valid = serde_json::json!({
        "schema_version": 1,
        "name": "search-and-extract",
        "steps": [
            {
                "type": "locate",
                "id": "locate_product",
                "query": "search button",
                "control": { "max_retries": 0 }
            },
            {
                "type": "verify",
                "id": "verify_product",
                "target": "search button",
                "expected": "enabled",
                "control": {}
            },
            {
                "type": "act",
                "id": "click_product",
                "action": "click",
                "target": "search button",
                "control": {}
            },
            {
                "type": "extract",
                "id": "extract_product",
                "key": "product_name",
                "selector": "#product-name",
                "control": {}
            }
        ]
    });

    validate_skill_json(&valid).expect("valid example should satisfy skill schema");

    let invalid = serde_json::json!({
        "schema_version": 1,
        "name": "invalid",
        "steps": [
            {
                "type": "act",
                "id": "missing_target",
                "action": "click",
                "control": {}
            }
        ]
    });

    assert!(
        validate_skill_json(&invalid).is_err(),
        "invalid skill json should fail schema validation"
    );
}

#[test]
fn test_examples_sample_skill_json_is_schema_valid() -> Result<()> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("examples/sample_skill.json");
    let text = fs::read_to_string(&path)
        .with_context(|| format!("examples/sample_skill.json not found at {}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).context("examples/sample_skill.json is not valid JSON")?;
    validate_skill_json(&value)
        .map_err(|e| anyhow::anyhow!("examples/sample_skill.json failed schema validation: {e}"))?;
    Ok(())
}

#[test]
fn test_examples_sample_skill_json_is_runtime_valid() -> Result<()> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("examples/sample_skill.json");
    let text = fs::read_to_string(&path)
        .with_context(|| format!("examples/sample_skill.json not found at {}", path.display()))?;
    let skill: SkillDefinition = serde_json::from_str(&text)
        .context("examples/sample_skill.json failed to deserialize as SkillDefinition")?;
    validate_skill_definition(&skill).map_err(|e| {
        anyhow::anyhow!("examples/sample_skill.json failed runtime validation: {e}")
    })?;
    Ok(())
}
