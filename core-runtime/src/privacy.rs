use regex::Regex;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::sre::state::{FastSemanticState, FullSemanticState, SemanticNode};

/// A named regex pattern for domain-specific PII detection.
///
/// Domain patterns are registered at startup and applied by `PiiRedactor`
/// alongside the built-in email and credit-card patterns.
pub struct DomainPattern {
    pub name: String,
    regex: Regex,
    replacement: String,
}

impl DomainPattern {
    /// Build a domain pattern.  Returns an error if `pattern` is not valid regex.
    pub fn new(
        name: impl Into<String>,
        pattern: &str,
        replacement: impl Into<String>,
    ) -> Result<Self, regex::Error> {
        Ok(Self {
            name: name.into(),
            regex: Regex::new(pattern)?,
            replacement: replacement.into(),
        })
    }
}

/// Centralized, forced-hook PII redactor.
///
/// Two hook points are mandated by the spec (Section 3.8):
/// - Exit of the SRE Queue (`redact_semantic_*` methods).
/// - Entry of the Audit Sink (`redact_json` / `redact_text` methods).
///
/// Domain-specific patterns can be added at startup to support the Wasm plugin
/// extension point described in ISSUE-17.
pub struct PiiRedactor {
    domain_patterns: Vec<DomainPattern>,
}

impl PiiRedactor {
    /// Construct a redactor with only the built-in patterns.
    pub fn new() -> Self {
        Self {
            domain_patterns: Vec::new(),
        }
    }

    /// Construct a redactor with additional domain-specific patterns.
    pub fn with_domain_patterns(domain_patterns: Vec<DomainPattern>) -> Self {
        Self { domain_patterns }
    }

    // -------------------------------------------------------------------------
    // Text-level redaction
    // -------------------------------------------------------------------------

    /// Redact PII embedded in freeform text (email addresses, card numbers,
    /// then domain patterns).
    pub fn redact_text(&self, text: &str) -> String {
        static CC_RE: OnceLock<Regex> = OnceLock::new();
        static EMAIL_RE: OnceLock<Regex> = OnceLock::new();

        let cc_re = CC_RE
            .get_or_init(|| Regex::new(r"\b\d(?:[ -]?\d){12,18}\b").expect("Invalid CC regex"));
        let email_re = EMAIL_RE.get_or_init(|| {
            Regex::new(r"(?i)\b[a-z0-9._%+\-]+@[a-z0-9.\-]+\.[a-z]{2,}\b")
                .expect("Invalid email regex")
        });

        let after_cc = cc_re.replace_all(text, "****-****-****-XXXX");
        let after_email = email_re.replace_all(after_cc.as_ref(), "***");

        // Apply domain-specific patterns sequentially.
        let mut result = after_email.into_owned();
        for dp in &self.domain_patterns {
            let replaced = dp.regex.replace_all(&result, dp.replacement.as_str());
            if matches!(replaced, std::borrow::Cow::Owned(_)) {
                result = replaced.into_owned();
            }
        }
        result
    }

    // -------------------------------------------------------------------------
    // JSON-level redaction
    // -------------------------------------------------------------------------

    /// Deep-redact a JSON value destined for the Audit Sink (state payloads).
    pub fn redact_json(&self, value: &Value) -> Value {
        self.redact_json_inner(value, None, false)
    }

    /// Deep-redact a JSON value representing tool-call arguments.
    ///
    /// In addition to key-based masking, this also masks `value` / `text`
    /// fields that commonly carry raw user input to interactive elements.
    pub fn redact_json_tool_args(&self, value: &Value) -> Value {
        self.redact_json_inner(value, None, true)
    }

    fn redact_json_inner(
        &self,
        value: &Value,
        key_hint: Option<&str>,
        mask_tool_value_field: bool,
    ) -> Value {
        match value {
            Value::Object(map) => {
                let mut out = serde_json::Map::with_capacity(map.len());
                for (key, child) in map {
                    let key_lower = key.to_ascii_lowercase();
                    let masked = if Self::is_sensitive_key(&key_lower)
                        || (mask_tool_value_field && Self::is_tool_value_key(&key_lower))
                    {
                        Value::String("***".to_string())
                    } else {
                        self.redact_json_inner(
                            child,
                            Some(key_lower.as_str()),
                            mask_tool_value_field,
                        )
                    };
                    out.insert(key.clone(), masked);
                }
                Value::Object(out)
            }
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| self.redact_json_inner(item, key_hint, mask_tool_value_field))
                    .collect(),
            ),
            Value::String(text) => {
                if key_hint.is_some_and(Self::is_sensitive_key) {
                    Value::String("***".to_string())
                } else {
                    Value::String(self.redact_text(text))
                }
            }
            _ => value.clone(),
        }
    }

    // -------------------------------------------------------------------------
    // SRE state redaction (exit hook for SRE Queue)
    // -------------------------------------------------------------------------

    /// Redact PII in a `FastSemanticState` before it leaves the SRE queue.
    pub fn redact_fast_state(&self, state: FastSemanticState) -> FastSemanticState {
        FastSemanticState {
            interactive_elements: state
                .interactive_elements
                .into_iter()
                .map(|n| self.redact_node(n))
                .collect(),
            messages: state
                .messages
                .into_iter()
                .map(|n| self.redact_node(n))
                .collect(),
        }
    }

    /// Redact PII in a `FullSemanticState` before it leaves the SRE queue.
    pub fn redact_full_state(&self, state: FullSemanticState) -> FullSemanticState {
        FullSemanticState {
            forms: state
                .forms
                .into_iter()
                .map(|n| self.redact_node(n))
                .collect(),
            regions: state
                .regions
                .into_iter()
                .map(|n| self.redact_node(n))
                .collect(),
        }
    }

    fn redact_node(&self, node: SemanticNode) -> SemanticNode {
        let label = node.label.map(|l| {
            if Self::is_sensitive_role(&node.role) {
                "***".to_string()
            } else {
                self.redact_text(&l)
            }
        });

        let alias = node.alias.map(|a| self.redact_text(&a));

        let attributes = node.attributes.map(|attrs| {
            // Detect context: if any attribute declares a sensitive type (e.g.
            // type="password"), treat the `value` attribute as sensitive too.
            let has_sensitive_type = attrs
                .get("type")
                .is_some_and(|t| matches!(t.to_ascii_lowercase().as_str(), "password" | "hidden"));
            attrs
                .into_iter()
                .map(|(k, v)| {
                    let k_lower = k.to_ascii_lowercase();
                    let v_redacted = if Self::is_sensitive_key(&k_lower)
                        || (has_sensitive_type && k_lower == "value")
                    {
                        "***".to_string()
                    } else {
                        self.redact_text(&v)
                    };
                    (k, v_redacted)
                })
                .collect::<BTreeMap<_, _>>()
        });

        SemanticNode {
            role: node.role,
            label,
            children: node
                .children
                .into_iter()
                .map(|c| self.redact_node(c))
                .collect(),
            attributes,
            stable_key: node.stable_key,
            ambiguous: node.ambiguous,
            alias,
            backend_node_id: node.backend_node_id,
            security_flags: node.security_flags,
        }
    }

    // -------------------------------------------------------------------------
    // Key classification helpers (pub(crate) for tests in audit.rs)
    // -------------------------------------------------------------------------

    pub(crate) fn is_sensitive_key(key: &str) -> bool {
        matches!(
            key,
            "password"
                | "passwd"
                | "email"
                | "token"
                | "secret"
                | "authorization"
                | "auth"
                | "card"
                | "credit_card"
                | "cc"
                | "cvv"
                | "cvc"
        ) || key.contains("password")
            || key.contains("email")
            || key.contains("token")
            || key.contains("secret")
            || key.contains("card")
    }

    fn is_tool_value_key(key: &str) -> bool {
        key == "value" || key == "text" || key.ends_with("_text")
    }

    /// Returns true for HTML roles that typically carry sensitive user input.
    fn is_sensitive_role(role: &str) -> bool {
        matches!(role, "password" | "credit-card" | "creditcard")
    }
}

impl Default for PiiRedactor {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Global default instance
// ---------------------------------------------------------------------------

static GLOBAL_REDACTOR: OnceLock<PiiRedactor> = OnceLock::new();

/// Return a reference to the process-wide default `PiiRedactor`.
///
/// This is the instance used by both the Audit Sink and SRE Queue hooks.
/// Call `register_global_redactor` before the first pipeline invocation to
/// swap in a redactor with domain-specific patterns.
pub fn global() -> &'static PiiRedactor {
    GLOBAL_REDACTOR.get_or_init(PiiRedactor::new)
}

/// Register a custom redactor as the global instance.
///
/// **Must be called before any pipeline is constructed** (i.e. before the first
/// call to `AsyncPipeline::new` or `AuditLogger::log`). Both call `global()`
/// on first use, locking in the default bare `PiiRedactor`; any registration
/// attempt after that point is silently discarded.
///
/// Returns `Ok(())` on success or `Err(redactor)` if the global was already
/// initialised (indicating a too-late call — log a warning and investigate the
/// startup ordering).
pub fn register_global_redactor(redactor: PiiRedactor) -> Result<(), PiiRedactor> {
    match GLOBAL_REDACTOR.set(redactor) {
        Ok(()) => Ok(()),
        Err(r) => {
            eprintln!(
                "[PRIVACY][WARN] register_global_redactor called after global() was already \
                 initialised — domain-specific PII patterns will NOT take effect. \
                 Call register_global_redactor before constructing AsyncPipeline or AuditLogger."
            );
            Err(r)
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn redactor() -> PiiRedactor {
        PiiRedactor::new()
    }

    // -- redact_text ----------------------------------------------------------

    #[test]
    fn redact_text_masks_email() {
        let r = redactor();
        assert_eq!(
            r.redact_text("contact alice@example.com now"),
            "contact *** now"
        );
    }

    #[test]
    fn redact_text_masks_credit_card() {
        let r = redactor();
        assert_eq!(
            r.redact_text("Card 4111-1111-1111-1111 ok"),
            "Card ****-****-****-XXXX ok"
        );
    }

    #[test]
    fn redact_text_masks_both_in_one_string() {
        let r = redactor();
        let input = "Card 4111-1111-1111-1111 for alice@example.com";
        let output = r.redact_text(input);
        assert_eq!(output, "Card ****-****-****-XXXX for ***");
    }

    #[test]
    fn redact_text_no_pii_returns_same_content() {
        let r = redactor();
        let input = "Hello world, no PII here";
        assert_eq!(r.redact_text(input), input);
    }

    #[test]
    fn redact_text_applies_domain_pattern() {
        let dp = DomainPattern::new("ssn", r"\b\d{3}-\d{2}-\d{4}\b", "[SSN]").unwrap();
        let r = PiiRedactor::with_domain_patterns(vec![dp]);
        assert_eq!(r.redact_text("SSN is 123-45-6789"), "SSN is [SSN]");
    }

    // -- redact_json ----------------------------------------------------------

    #[test]
    fn redact_json_masks_sensitive_keys() {
        let r = redactor();
        let input =
            json!({ "username": "alice", "password": "s3cr3t", "email": "alice@example.com" });
        let output = r.redact_json(&input);
        assert_eq!(output["username"], "alice");
        assert_eq!(output["password"], "***");
        assert_eq!(output["email"], "***");
    }

    #[test]
    fn redact_json_masks_inline_pii_in_string_values() {
        let r = redactor();
        let input = json!({ "message": "send to alice@example.com" });
        let output = r.redact_json(&input);
        assert_eq!(output["message"], "send to ***");
    }

    #[test]
    fn redact_json_recurses_into_arrays() {
        let r = redactor();
        let input = json!([{ "password": "x" }, { "ok": "y" }]);
        let output = r.redact_json(&input);
        assert_eq!(output[0]["password"], "***");
        assert_eq!(output[1]["ok"], "y");
    }

    // -- redact_json_tool_args ------------------------------------------------

    #[test]
    fn redact_json_tool_args_masks_value_and_text_fields() {
        let r = redactor();
        let input = json!({ "selector": "#email", "value": "alice@example.com" });
        let output = r.redact_json_tool_args(&input);
        assert_eq!(output["selector"], "#email");
        assert_eq!(output["value"], "***");
    }

    // -- redact_fast_state ----------------------------------------------------

    #[test]
    fn redact_fast_state_masks_node_labels_with_pii() {
        let r = redactor();
        let node = SemanticNode {
            role: "input".to_string(),
            label: Some("alice@example.com".to_string()),
            children: vec![],
            attributes: None,
            stable_key: None,
            ambiguous: false,
            alias: None,
            backend_node_id: 0,
            security_flags: vec![],
        };
        let state = FastSemanticState {
            interactive_elements: vec![node],
            messages: vec![],
        };
        let redacted = r.redact_fast_state(state);
        assert_eq!(
            redacted.interactive_elements[0].label.as_deref(),
            Some("***")
        );
    }

    #[test]
    fn redact_fast_state_masks_sensitive_attribute_values() {
        let r = redactor();
        let mut attrs = BTreeMap::new();
        attrs.insert("name".to_string(), "email".to_string());
        attrs.insert("value".to_string(), "user@example.com".to_string());
        let node = SemanticNode {
            role: "input".to_string(),
            label: None,
            children: vec![],
            attributes: Some(attrs),
            stable_key: None,
            ambiguous: false,
            alias: None,
            backend_node_id: 0,
            security_flags: vec![],
        };
        let state = FastSemanticState {
            interactive_elements: vec![node],
            messages: vec![],
        };
        let redacted = r.redact_fast_state(state);
        let attrs = redacted.interactive_elements[0]
            .attributes
            .as_ref()
            .unwrap();
        // "name" key is not sensitive; "value" attribute key is not either,
        // but the value itself contains an email pattern → should be masked.
        assert_eq!(attrs["value"], "***");
    }

    #[test]
    fn redact_full_state_masks_nodes_in_forms() {
        let r = redactor();
        let node = SemanticNode {
            role: "input".to_string(),
            label: Some("Card 4111-1111-1111-1111".to_string()),
            children: vec![],
            attributes: None,
            stable_key: None,
            ambiguous: false,
            alias: None,
            backend_node_id: 0,
            security_flags: vec![],
        };
        let state = FullSemanticState {
            forms: vec![node],
            regions: vec![],
        };
        let redacted = r.redact_full_state(state);
        assert_eq!(
            redacted.forms[0].label.as_deref(),
            Some("Card ****-****-****-XXXX")
        );
    }

    // -- is_sensitive_key -----------------------------------------------------

    #[test]
    fn is_sensitive_key_catches_substrings() {
        assert!(PiiRedactor::is_sensitive_key("user_password"));
        assert!(PiiRedactor::is_sensitive_key("email_address"));
        assert!(PiiRedactor::is_sensitive_key("api_token"));
        assert!(!PiiRedactor::is_sensitive_key("username"));
        assert!(!PiiRedactor::is_sensitive_key("role"));
    }
}
