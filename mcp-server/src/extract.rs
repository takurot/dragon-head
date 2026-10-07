use super::*;

/// Longest page-derived JavaScript exception text echoed in a `ScriptEvalError`.
pub(crate) const EXTRACT_ERROR_MESSAGE_MAX_CHARS: usize = 300;

/// Unpacks the `{ ok, value | error }` envelope from `ExtractionRule::to_js_checked_script`.
/// The `Err` text is the page's own exception message: untrusted, see [`safe_script_error_message`].
pub(crate) fn unwrap_extraction_envelope(envelope: Value) -> std::result::Result<Value, String> {
    match envelope.get("ok").and_then(Value::as_bool) {
        Some(true) => Ok(envelope.get("value").cloned().unwrap_or(Value::Null)),
        Some(false) => Err(envelope
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("unknown JavaScript error")
            .to_string()),
        None => Err("extraction script returned an unexpected shape".to_string()),
    }
}

/// A page can override DOM builtins and so control the exception text, which makes it an
/// injection channel that bypasses result sanitization. Cap its length, and withhold it entirely
/// when the sanitizer flags it (an `Err` cannot carry `security_flags`).
pub(crate) fn safe_script_error_message(sanitizer: &PromptInjectionSanitizer, raw: &str) -> String {
    let capped: String = raw.chars().take(EXTRACT_ERROR_MESSAGE_MAX_CHARS).collect();
    let (text, flags) = sanitizer.sanitize_json_value(Value::String(capped));
    if !flags.is_empty() {
        return "message withheld: flagged by the prompt-injection sanitizer".to_string();
    }
    let redacted = core_runtime::privacy::global().redact_json(&text);
    redacted.as_str().unwrap_or_default().to_string()
}

/// Diagnostics come from a script running in the (untrusted) page, so they get the same
/// sanitization and PII redaction as extracted content. Only string values are kept.
pub(crate) fn sanitize_extraction_errors(
    sanitizer: &PromptInjectionSanitizer,
    errors: serde_json::Map<String, Value>,
) -> (serde_json::Map<String, Value>, Vec<String>) {
    let strings: serde_json::Map<String, Value> = errors
        .into_iter()
        .filter(|(_, value)| value.is_string())
        .collect();
    let (sanitized, flags) = sanitizer.sanitize_json_value(Value::Object(strings));
    let redacted = core_runtime::privacy::global().redact_json(&sanitized);
    match redacted {
        Value::Object(map) => (map, flags),
        _ => (serde_json::Map::new(), flags),
    }
}

/// Whether an extraction result is worth diagnosing: nothing matched (`null`, `[]`) or a
/// structured row has an unresolved (`null`) field.
pub(crate) fn extraction_result_has_gaps(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Array(rows) => {
            rows.is_empty()
                || rows.iter().any(|row| {
                    row.as_object()
                        .is_some_and(|o| o.values().any(Value::is_null))
                })
        }
        _ => false,
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtractArguments {
    #[serde(default)]
    pub(crate) rule_name: Option<String>,
    #[serde(default)]
    pub(crate) inline: Option<Value>,
    /// Also return the generated JavaScript (ISSUE-257).
    #[serde(default)]
    pub(crate) debug: bool,
}
