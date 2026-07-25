use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionMode {
    Single {
        selector: String,
        #[serde(default)]
        attribute: Option<String>,
    },
    Structured {
        selector: String,
        fields: HashMap<String, String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractionRule {
    pub name: String,
    pub mode: ExtractionMode,
}

impl ExtractionRule {
    pub fn from_value(name: &str, value: &Value) -> Result<Self, ExtractionRuleError> {
        let obj = value.as_object().ok_or(ExtractionRuleError::InvalidFormat(
            "root must be a JSON object".into(),
        ))?;

        // items: { selector, fields } form (matches SPEC DSL)
        if let Some(items) = obj.get("items") {
            let items_obj = items.as_object().ok_or(ExtractionRuleError::InvalidFormat(
                "'items' must be a JSON object".into(),
            ))?;

            let selector = items_obj
                .get("selector")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or(ExtractionRuleError::MissingField("items.selector"))?
                .to_string();

            let fields_val = items_obj
                .get("fields")
                .ok_or(ExtractionRuleError::MissingField("items.fields"))?;

            let fields = parse_fields_map(fields_val)?;

            return Ok(ExtractionRule {
                name: name.to_string(),
                mode: ExtractionMode::Structured { selector, fields },
            });
        }

        // { selector, fields } flat form
        if let Some(fields_val) = obj.get("fields") {
            let selector = obj
                .get("selector")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or(ExtractionRuleError::MissingField("selector"))?
                .to_string();

            let fields = parse_fields_map(fields_val)?;

            return Ok(ExtractionRule {
                name: name.to_string(),
                mode: ExtractionMode::Structured { selector, fields },
            });
        }

        // Single value form: { selector, attribute? }
        let selector = obj
            .get("selector")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or(ExtractionRuleError::MissingField("selector"))?
            .to_string();

        let attribute = obj
            .get("attribute")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);

        Ok(ExtractionRule {
            name: name.to_string(),
            mode: ExtractionMode::Single {
                selector,
                attribute,
            },
        })
    }

    /// Generate a JavaScript IIFE that extracts data from the DOM.
    /// Returns JSON-serializable output: a string for Single mode, an array of objects for Structured.
    pub fn to_js_script(&self) -> String {
        match &self.mode {
            ExtractionMode::Single {
                selector,
                attribute,
            } => {
                let sel_json =
                    serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".to_string());
                match attribute.as_deref() {
                    None | Some("text") => format!(
                        "(() => {{ const el = document.querySelector({sel_json}); \
                         return el ? (el.innerText || el.textContent || '').trim() : null; }})()"
                    ),
                    Some(attr) if is_dom_property(attr) => format!(
                        "(() => {{ const el = document.querySelector({sel_json}); \
                         return el ? (el.{attr} ?? null) : null; }})()"
                    ),
                    Some(attr) => {
                        let attr_json =
                            serde_json::to_string(attr).unwrap_or_else(|_| "\"\"".to_string());
                        format!(
                            "(() => {{ const el = document.querySelector({sel_json}); \
                             return el ? el.getAttribute({attr_json}) : null; }})()"
                        )
                    }
                }
            }
            ExtractionMode::Structured { selector, fields } => {
                let sel_json =
                    serde_json::to_string(selector).unwrap_or_else(|_| "\"\"".to_string());

                let field_lines: Vec<String> = fields
                    .iter()
                    .map(|(key, field_sel)| {
                        let key_json =
                            serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string());
                        let field_sel_json =
                            serde_json::to_string(field_sel).unwrap_or_else(|_| "\"\"".to_string());
                        format!(
                            "{key_json}: (() => {{ const f = item.querySelector({field_sel_json}); \
                             return f ? (f.innerText || f.textContent || '').trim() : null; }})()"
                        )
                    })
                    .collect();

                let fields_obj = field_lines.join(", ");

                format!(
                    "(() => {{ \
                     const items = Array.from(document.querySelectorAll({sel_json})); \
                     return items.map(item => ({{ {fields_obj} }})); \
                     }})()"
                )
            }
        }
    }
}

/// DOM properties that must be accessed via `.propName`, not `.getAttribute("propName")` —
/// `getAttribute` only reads the HTML attribute string and returns null/stale values for
/// these, since they reflect live JS-side element state.
fn is_dom_property(attr: &str) -> bool {
    matches!(attr, "textContent" | "innerText" | "value")
}

fn parse_fields_map(value: &Value) -> Result<HashMap<String, String>, ExtractionRuleError> {
    let obj = value.as_object().ok_or(ExtractionRuleError::InvalidFormat(
        "'fields' must be a JSON object".into(),
    ))?;

    if obj.is_empty() {
        return Err(ExtractionRuleError::InvalidFormat(
            "'fields' must have at least one entry".into(),
        ));
    }

    let mut map = HashMap::new();
    for (key, val) in obj {
        let selector = val
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ExtractionRuleError::InvalidFormat(format!(
                    "field '{key}' value must be a non-empty CSS selector string"
                ))
            })?
            .to_string();
        map.insert(key.clone(), selector);
    }
    Ok(map)
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExtractionRuleError {
    #[error("invalid DSL format: {0}")]
    InvalidFormat(String),
    #[error("required field missing: {0}")]
    MissingField(&'static str),
    #[error("rule '{0}' is already registered")]
    DuplicateRule(String),
}

/// Pre-compiled extraction rule registry.
///
/// Rules are parsed and validated at registration time (`register`),
/// eliminating parse overhead at execution time.
#[derive(Debug, Clone, Default)]
pub struct SchemaRegistry {
    rules: HashMap<String, ExtractionRule>,
}

impl SchemaRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parse and register an extraction rule from a JSON value.
    /// Returns `ExtractionRuleError::DuplicateRule` if a rule with the same name already exists.
    pub fn register(&mut self, name: &str, value: &Value) -> Result<(), ExtractionRuleError> {
        if self.rules.contains_key(name) {
            return Err(ExtractionRuleError::DuplicateRule(name.to_string()));
        }
        let rule = ExtractionRule::from_value(name, value)?;
        self.rules.insert(name.to_string(), rule);
        Ok(())
    }

    /// Look up a pre-compiled rule by name.
    pub fn get(&self, name: &str) -> Option<&ExtractionRule> {
        self.rules.get(name)
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// Return all golden fixture rule paths discovered under `core-runtime/tests/fixtures/golden/`.
/// Each entry is `(rule_json_path, expected_json_path)`.
///
/// Used by the golden accuracy tests.
#[cfg(test)]
fn discover_golden_fixtures() -> Vec<(std::path::PathBuf, std::path::PathBuf)> {
    let fixtures_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("core-runtime/tests/fixtures/golden");

    let mut pairs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&fixtures_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if !stem.ends_with("_rule") {
                continue;
            }
            let base = stem.trim_end_matches("_rule");
            let expected_path = fixtures_dir.join(format!("{base}_expected.json"));
            if expected_path.exists() {
                pairs.push((path, expected_path));
            }
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // --- ExtractionRule::from_value ---

    #[test]
    fn parses_single_selector_rule() {
        let value = json!({ "selector": "h1.title" });
        let rule = ExtractionRule::from_value("title", &value).unwrap();
        assert_eq!(rule.name, "title");
        assert_eq!(
            rule.mode,
            ExtractionMode::Single {
                selector: "h1.title".into(),
                attribute: None,
            }
        );
    }

    #[test]
    fn parses_single_rule_with_attribute() {
        let value = json!({ "selector": "a.link", "attribute": "href" });
        let rule = ExtractionRule::from_value("link", &value).unwrap();
        assert_eq!(
            rule.mode,
            ExtractionMode::Single {
                selector: "a.link".into(),
                attribute: Some("href".into()),
            }
        );
    }

    #[test]
    fn parses_structured_rule_with_items_key() {
        let value = json!({
            "items": {
                "selector": "tr.product",
                "fields": {
                    "price": ".amt",
                    "name": ".title"
                }
            }
        });
        let rule = ExtractionRule::from_value("products", &value).unwrap();
        assert_eq!(rule.name, "products");
        let ExtractionMode::Structured { selector, fields } = rule.mode else {
            panic!("expected Structured mode");
        };
        assert_eq!(selector, "tr.product");
        assert_eq!(fields.get("price").map(String::as_str), Some(".amt"));
        assert_eq!(fields.get("name").map(String::as_str), Some(".title"));
    }

    #[test]
    fn parses_structured_rule_flat_form() {
        let value = json!({
            "selector": "li.item",
            "fields": {
                "label": ".lbl"
            }
        });
        let rule = ExtractionRule::from_value("items", &value).unwrap();
        let ExtractionMode::Structured { selector, .. } = rule.mode else {
            panic!("expected Structured mode");
        };
        assert_eq!(selector, "li.item");
    }

    #[test]
    fn rejects_missing_selector() {
        let value = json!({ "attribute": "href" });
        let err = ExtractionRule::from_value("x", &value).unwrap_err();
        assert!(matches!(err, ExtractionRuleError::MissingField("selector")));
    }

    #[test]
    fn rejects_empty_selector() {
        let value = json!({ "selector": "" });
        let err = ExtractionRule::from_value("x", &value).unwrap_err();
        assert!(matches!(err, ExtractionRuleError::MissingField("selector")));
    }

    #[test]
    fn rejects_non_object_root() {
        let value = json!("not an object");
        let err = ExtractionRule::from_value("x", &value).unwrap_err();
        assert!(matches!(err, ExtractionRuleError::InvalidFormat(_)));
    }

    #[test]
    fn rejects_empty_fields_map() {
        let value = json!({ "selector": "li", "fields": {} });
        let err = ExtractionRule::from_value("x", &value).unwrap_err();
        assert!(matches!(err, ExtractionRuleError::InvalidFormat(_)));
    }

    #[test]
    fn rejects_non_string_field_selector() {
        let value = json!({ "selector": "li", "fields": { "key": 42 } });
        let err = ExtractionRule::from_value("x", &value).unwrap_err();
        assert!(matches!(err, ExtractionRuleError::InvalidFormat(_)));
    }

    // --- ExtractionRule::to_js_script ---

    #[test]
    fn single_rule_generates_innertext_script() {
        let rule = ExtractionRule {
            name: "title".into(),
            mode: ExtractionMode::Single {
                selector: "h1".into(),
                attribute: None,
            },
        };
        let js = rule.to_js_script();
        assert!(js.contains("querySelector(\"h1\")"));
        assert!(js.contains("innerText"));
        assert!(!js.contains("getAttribute"));
    }

    #[test]
    fn single_rule_with_attribute_generates_getattribute_script() {
        let rule = ExtractionRule {
            name: "link".into(),
            mode: ExtractionMode::Single {
                selector: "a.link".into(),
                attribute: Some("href".into()),
            },
        };
        let js = rule.to_js_script();
        assert!(js.contains("getAttribute(\"href\")"));
        assert!(!js.contains("innerText"));
    }

    #[test]
    fn structured_rule_generates_queryselectorall_script() {
        let mut fields = HashMap::new();
        fields.insert("price".to_string(), ".amt".to_string());
        let rule = ExtractionRule {
            name: "products".into(),
            mode: ExtractionMode::Structured {
                selector: "tr.product".into(),
                fields,
            },
        };
        let js = rule.to_js_script();
        assert!(js.contains("querySelectorAll(\"tr.product\")"));
        assert!(js.contains("querySelector(\".amt\")"));
        assert!(js.contains("\"price\""));
        assert!(js.contains("items.map"));
    }

    #[test]
    fn single_rule_with_text_content_attribute_generates_property_access() {
        let rule = ExtractionRule {
            name: "heading".into(),
            mode: ExtractionMode::Single {
                selector: "h1".into(),
                attribute: Some("textContent".into()),
            },
        };
        let js = rule.to_js_script();
        assert!(js.contains(".textContent"));
        assert!(!js.contains("getAttribute"));
    }

    #[test]
    fn single_rule_with_inner_text_attribute_generates_property_access() {
        let rule = ExtractionRule {
            name: "heading".into(),
            mode: ExtractionMode::Single {
                selector: "h1".into(),
                attribute: Some("innerText".into()),
            },
        };
        let js = rule.to_js_script();
        assert!(js.contains(".innerText"));
        assert!(!js.contains("getAttribute"));
    }

    #[test]
    fn single_rule_with_value_attribute_generates_property_access() {
        let rule = ExtractionRule {
            name: "input_value".into(),
            mode: ExtractionMode::Single {
                selector: "input".into(),
                attribute: Some("value".into()),
            },
        };
        let js = rule.to_js_script();
        assert!(js.contains(".value"));
        assert!(!js.contains("getAttribute"));
    }

    #[test]
    fn single_rule_with_dom_property_coalesces_undefined_to_null() {
        let rule = ExtractionRule {
            name: "heading".into(),
            mode: ExtractionMode::Single {
                selector: "h1".into(),
                attribute: Some("value".into()),
            },
        };
        let js = rule.to_js_script();
        // Elements without the requested property (e.g. `value` on a <div>) yield
        // `undefined`, which is not valid JSON and breaks evaluate_script_json.
        assert!(js.contains("el.value ?? null"));
    }

    #[test]
    fn single_rule_with_href_attribute_still_uses_get_attribute() {
        let rule = ExtractionRule {
            name: "link".into(),
            mode: ExtractionMode::Single {
                selector: "a".into(),
                attribute: Some("href".into()),
            },
        };
        let js = rule.to_js_script();
        assert!(js.contains("getAttribute(\"href\")"));
    }

    #[test]
    fn single_text_attribute_generates_same_as_no_attribute() {
        let rule_none = ExtractionRule {
            name: "x".into(),
            mode: ExtractionMode::Single {
                selector: "p".into(),
                attribute: None,
            },
        };
        let rule_text = ExtractionRule {
            name: "x".into(),
            mode: ExtractionMode::Single {
                selector: "p".into(),
                attribute: Some("text".into()),
            },
        };
        assert_eq!(rule_none.to_js_script(), rule_text.to_js_script());
    }

    // --- SchemaRegistry ---

    #[test]
    fn registry_starts_empty() {
        let reg = SchemaRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn registry_register_and_get() {
        let mut reg = SchemaRegistry::new();
        let value = json!({ "selector": "h1" });
        reg.register("title", &value).unwrap();
        assert_eq!(reg.len(), 1);
        let rule = reg.get("title").unwrap();
        assert_eq!(rule.name, "title");
    }

    #[test]
    fn registry_get_unknown_returns_none() {
        let reg = SchemaRegistry::new();
        assert!(reg.get("unknown").is_none());
    }

    #[test]
    fn registry_rejects_duplicate_name() {
        let mut reg = SchemaRegistry::new();
        let value = json!({ "selector": "h1" });
        reg.register("title", &value).unwrap();
        let err = reg.register("title", &value).unwrap_err();
        assert!(matches!(err, ExtractionRuleError::DuplicateRule(_)));
    }

    #[test]
    fn registry_propagates_parse_error() {
        let mut reg = SchemaRegistry::new();
        let value = json!("not an object");
        let err = reg.register("bad", &value).unwrap_err();
        assert!(matches!(err, ExtractionRuleError::InvalidFormat(_)));
    }

    #[test]
    fn registry_multiple_rules() {
        let mut reg = SchemaRegistry::new();
        reg.register("a", &json!({ "selector": "h1" })).unwrap();
        reg.register("b", &json!({ "selector": "p" })).unwrap();
        assert_eq!(reg.len(), 2);
        assert!(reg.get("a").is_some());
        assert!(reg.get("b").is_some());
    }

    // --- Golden Dataset accuracy: all fixtures must parse without error (100% parse accuracy) ---

    #[test]
    fn golden_fixtures_all_parse_successfully() {
        let pairs = discover_golden_fixtures();
        assert!(
            !pairs.is_empty(),
            "no golden fixtures found in core-runtime/tests/fixtures/golden/"
        );

        let mut failures: Vec<String> = Vec::new();
        for (rule_path, _expected_path) in &pairs {
            let rule_json_str = std::fs::read_to_string(rule_path)
                .unwrap_or_else(|e| panic!("failed to read {rule_path:?}: {e}"));
            let rule_value: Value = serde_json::from_str(&rule_json_str)
                .unwrap_or_else(|e| panic!("invalid JSON in {rule_path:?}: {e}"));

            let stem = rule_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");
            let name = stem.trim_end_matches("_rule");

            if let Err(err) = ExtractionRule::from_value(name, &rule_value) {
                failures.push(format!("{name}: {err}"));
            }
        }

        assert!(
            failures.is_empty(),
            "golden fixture parse failures (accuracy < 100%):\n{}",
            failures.join("\n")
        );
    }

    #[test]
    fn golden_fixtures_all_generate_valid_js() {
        let pairs = discover_golden_fixtures();
        let mut failures: Vec<String> = Vec::new();

        for (rule_path, _) in &pairs {
            let rule_json_str = std::fs::read_to_string(rule_path)
                .unwrap_or_else(|e| panic!("failed to read {rule_path:?}: {e}"));
            let rule_value: Value = serde_json::from_str(&rule_json_str).unwrap();

            let stem = rule_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown");
            let name = stem.trim_end_matches("_rule");

            let Ok(rule) = ExtractionRule::from_value(name, &rule_value) else {
                continue;
            };

            let js = rule.to_js_script();
            // Generated JS must be a non-empty IIFE
            if !js.contains("(()") && !js.starts_with("(()") {
                failures.push(format!("{name}: generated JS is not an IIFE"));
            }
            if !js.contains("querySelector") {
                failures.push(format!("{name}: generated JS missing querySelector call"));
            }
        }

        assert!(
            failures.is_empty(),
            "golden JS generation failures:\n{}",
            failures.join("\n")
        );
    }
}
