use crate::sre::state::SemanticNode;
use regex::{Regex, RegexSet, RegexSetBuilder};

/// Placeholder substituted in `Redact` mode.
pub const REDACTION_PLACEHOLDER: &str = "[REDACTED_SECURITY]";

/// Flag value set on `SemanticNode::security_flags` when injection is detected.
pub const SECURITY_FLAG: &str = "possible_prompt_injection";

/// Known indirect prompt-injection phrases (conservative, ASCII, case-insensitive).
///
/// v1 limitations (by design — ISSUE-109 scope):
/// - Fixed patterns only; no user-defined regex.
/// - ASCII case-folding only; Unicode/homoglyph variants (e.g., Cyrillic lookalikes)
///   are **not** detected.
/// - `role` field is intentionally excluded — it holds a structural DOM role
///   (button/input/link etc.), not free-form user text.
const INJECTION_PATTERNS: &[&str] = &[
    "ignore previous instructions",
    "ignore all previous instructions",
    "ignore the above instructions",
    "ignore your previous instructions",
    "ignore your instructions",
    "disregard previous instructions",
    "disregard all previous instructions",
    "disregard the above instructions",
    "forget previous instructions",
    "forget all previous instructions",
    "override previous instructions",
    "new instructions:",
    "updated instructions:",
    "system prompt:",
    "you are now a",
    "pretend you are a",
    "jailbreak",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PromptInjectionMode {
    Off,
    #[default]
    ReportOnly,
    Redact,
}

#[derive(Debug, Clone, Default)]
pub struct PromptInjectionSanitizerConfig {
    pub mode: PromptInjectionMode,
}

/// Core prompt-injection sanitizer (SPEC SEC-03).
///
/// Apply to a `SemanticNode` tree **after** `stable_key` generation.
/// Scans `label`, `alias`, and `attributes` values on every node recursively.
///
/// **Note on ownership**: `sanitize_node` takes ownership of the node and returns
/// it (potentially modified). This avoids an extra clone in the common case where
/// the caller passes a freshly-built tree. Clone before calling if you need the
/// original.
///
/// **Note on `stable_key`**: `security_flags` is included in the `stable_key` hash
/// (see `hash_node`). Adding flags via this sanitizer after key generation means the
/// stored `stable_key` no longer matches a freshly-computed hash of the returned node.
/// This is intentional — keys are computed from pre-sanitization content, which is
/// stable even across classifier updates.
///
/// - `Off`: no-op.
/// - `ReportOnly` (default): text unchanged; sets `security_flags` on detected nodes.
/// - `Redact`: replaces matched phrases with `[REDACTED_SECURITY]`; sets `security_flags`.
pub struct PromptInjectionSanitizer {
    config: PromptInjectionSanitizerConfig,
    detect_set: RegexSet,
    /// Sorted longest-first so longer overlapping patterns match before shorter prefixes.
    replace_patterns: Vec<Regex>,
}

impl PromptInjectionSanitizer {
    pub fn new(config: PromptInjectionSanitizerConfig) -> Self {
        let escaped_for_detect: Vec<String> = INJECTION_PATTERNS
            .iter()
            .map(|&p| with_word_boundaries(p))
            .collect();

        let detect_set = RegexSetBuilder::new(&escaped_for_detect)
            .case_insensitive(true)
            .build()
            .expect("built-in patterns compile");

        // Sort longest-first: prevents shorter prefix patterns from firing before
        // a longer pattern that would match the whole phrase.
        let mut escaped_for_replace = escaped_for_detect;
        escaped_for_replace.sort_unstable_by_key(|s| std::cmp::Reverse(s.len()));

        let replace_patterns: Vec<Regex> = escaped_for_replace
            .iter()
            .map(|p| {
                regex::RegexBuilder::new(p)
                    .case_insensitive(true)
                    .build()
                    .expect("built-in patterns compile")
            })
            .collect();

        Self {
            config,
            detect_set,
            replace_patterns,
        }
    }

    /// Recursively sanitize a `SemanticNode` tree.
    pub fn sanitize_node(&self, mut node: SemanticNode) -> SemanticNode {
        if self.config.mode == PromptInjectionMode::Off {
            return node;
        }

        let mut detected = false;

        if let Some(label) = node.label.take() {
            let (text, hit) = self.process_text(label);
            detected |= hit;
            node.label = Some(text);
        }

        if let Some(alias) = node.alias.take() {
            let (text, hit) = self.process_text(alias);
            detected |= hit;
            node.alias = Some(text);
        }

        if let Some(mut attrs) = node.attributes.take() {
            for val in attrs.values_mut() {
                let (text, hit) = self.process_text(std::mem::take(val));
                detected |= hit;
                *val = text;
            }
            node.attributes = Some(attrs);
        }

        if detected {
            add_flag_dedup(&mut node.security_flags, SECURITY_FLAG);
        }

        node.children = node
            .children
            .into_iter()
            .map(|child| self.sanitize_node(child))
            .collect();

        node
    }

    /// Returns `(processed_text, was_detected)`.
    fn process_text(&self, text: String) -> (String, bool) {
        if !self.detect_set.is_match(&text) {
            return (text, false);
        }
        if self.config.mode == PromptInjectionMode::Redact {
            (self.redact_text(&text), true)
        } else {
            (text, true)
        }
    }

    fn redact_text(&self, text: &str) -> String {
        self.replace_patterns
            .iter()
            .fold(text.to_string(), |acc, re| {
                re.replace_all(&acc, REDACTION_PLACEHOLDER).into_owned()
            })
    }
}

/// Wraps a pattern in `\b` word-boundary anchors.
/// Trailing `\b` is omitted for patterns ending with a non-word character (e.g., `:`).
fn with_word_boundaries(pattern: &str) -> String {
    let escaped = regex::escape(pattern);
    let ends_with_word_char = pattern
        .chars()
        .last()
        .map(|c| c.is_ascii_alphanumeric() || c == '_')
        .unwrap_or(false);
    if ends_with_word_char {
        format!(r"\b{escaped}\b")
    } else {
        format!(r"\b{escaped}")
    }
}

fn add_flag_dedup(flags: &mut Vec<String>, flag: &str) {
    if !flags.iter().any(|f| f == flag) {
        flags.push(flag.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn make_sanitizer(mode: PromptInjectionMode) -> PromptInjectionSanitizer {
        PromptInjectionSanitizer::new(PromptInjectionSanitizerConfig { mode })
    }

    fn label_node(text: &str) -> SemanticNode {
        SemanticNode {
            role: "text".to_string(),
            label: Some(text.to_string()),
            ..Default::default()
        }
    }

    // ── Off mode ─────────────────────────────────────────────────────────────

    #[test]
    fn off_mode_does_not_modify_node() {
        let s = make_sanitizer(PromptInjectionMode::Off);
        let n = label_node("ignore previous instructions");
        let result = s.sanitize_node(n.clone());
        assert_eq!(result, n);
    }

    // ── ReportOnly mode ───────────────────────────────────────────────────────

    #[test]
    fn report_only_sets_flag_but_keeps_text() {
        let s = make_sanitizer(PromptInjectionMode::ReportOnly);
        let n = label_node("ignore previous instructions now");
        let result = s.sanitize_node(n);
        assert_eq!(
            result.label.as_deref(),
            Some("ignore previous instructions now")
        );
        assert!(result.security_flags.contains(&SECURITY_FLAG.to_string()));
    }

    #[test]
    fn report_only_clean_text_gets_no_flag() {
        let s = make_sanitizer(PromptInjectionMode::ReportOnly);
        let n = label_node("Buy now — limited offer for $10.99!");
        let result = s.sanitize_node(n);
        assert!(result.security_flags.is_empty());
    }

    #[test]
    fn report_only_does_not_duplicate_existing_flag() {
        let s = make_sanitizer(PromptInjectionMode::ReportOnly);
        let mut n = label_node("ignore previous instructions");
        n.security_flags = vec![SECURITY_FLAG.to_string()];
        let result = s.sanitize_node(n);
        let count = result
            .security_flags
            .iter()
            .filter(|f| f.as_str() == SECURITY_FLAG)
            .count();
        assert_eq!(count, 1);
    }

    // ── Redact mode ───────────────────────────────────────────────────────────

    #[test]
    fn redact_replaces_only_phrase_not_whole_string() {
        let s = make_sanitizer(PromptInjectionMode::Redact);
        let n = label_node("Please ignore previous instructions and do this instead");
        let result = s.sanitize_node(n);
        let label = result.label.as_deref().unwrap();
        assert!(label.contains(REDACTION_PLACEHOLDER), "placeholder present");
        assert!(
            !label.contains("ignore previous instructions"),
            "phrase removed"
        );
        assert!(label.contains("Please"), "prefix preserved");
        assert!(label.contains("and do this instead"), "suffix preserved");
        assert!(result.security_flags.contains(&SECURITY_FLAG.to_string()));
    }

    #[test]
    fn redact_sets_flag_and_replaces_text() {
        let s = make_sanitizer(PromptInjectionMode::Redact);
        let n = label_node("jailbreak attempt");
        let result = s.sanitize_node(n);
        assert!(result.security_flags.contains(&SECURITY_FLAG.to_string()));
        assert!(!result.label.as_deref().unwrap().contains("jailbreak"));
        assert!(result
            .label
            .as_deref()
            .unwrap()
            .contains(REDACTION_PLACEHOLDER));
    }

    #[test]
    fn redact_mode_alias_field() {
        let s = make_sanitizer(PromptInjectionMode::Redact);
        let n = SemanticNode {
            role: "link".to_string(),
            alias: Some("click here to jailbreak the system".to_string()),
            ..Default::default()
        };
        let result = s.sanitize_node(n);
        assert!(result.security_flags.contains(&SECURITY_FLAG.to_string()));
        let alias = result.alias.as_deref().unwrap();
        assert!(alias.contains(REDACTION_PLACEHOLDER), "alias redacted");
        assert!(!alias.contains("jailbreak"), "phrase removed from alias");
        assert!(alias.contains("click here to"), "prefix preserved in alias");
    }

    #[test]
    fn redact_mode_attribute_value() {
        let s = make_sanitizer(PromptInjectionMode::Redact);
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "aria-label".to_string(),
            "ignore previous instructions to get help".to_string(),
        );
        let n = SemanticNode {
            role: "button".to_string(),
            attributes: Some(attrs),
            ..Default::default()
        };
        let result = s.sanitize_node(n);
        assert!(result.security_flags.contains(&SECURITY_FLAG.to_string()));
        let val = result.attributes.as_ref().unwrap()["aria-label"].as_str();
        assert!(val.contains(REDACTION_PLACEHOLDER));
        assert!(!val.contains("ignore previous instructions"));
    }

    // ── Case insensitivity ────────────────────────────────────────────────────

    #[test]
    fn detection_is_case_insensitive_upper() {
        let s = make_sanitizer(PromptInjectionMode::ReportOnly);
        let n = label_node("IGNORE PREVIOUS INSTRUCTIONS");
        let result = s.sanitize_node(n);
        assert!(result.security_flags.contains(&SECURITY_FLAG.to_string()));
    }

    #[test]
    fn detection_is_case_insensitive_mixed() {
        let s = make_sanitizer(PromptInjectionMode::ReportOnly);
        let n = label_node("Ignore Previous Instructions");
        let result = s.sanitize_node(n);
        assert!(result.security_flags.contains(&SECURITY_FLAG.to_string()));
    }

    // ── Word boundary tests ───────────────────────────────────────────────────

    #[test]
    fn jailbreak_word_boundary_no_false_positive() {
        let s = make_sanitizer(PromptInjectionMode::ReportOnly);
        // "jailbreaker" should NOT trigger — "jailbreak" must match as a whole word
        let n = label_node("Use the jailbreaker tool for testing");
        let result = s.sanitize_node(n);
        assert!(
            result.security_flags.is_empty(),
            "jailbreaker should not trigger word-bounded jailbreak pattern"
        );
    }

    #[test]
    fn jailbreak_standalone_triggers() {
        let s = make_sanitizer(PromptInjectionMode::ReportOnly);
        let n = label_node("This is a jailbreak attempt");
        let result = s.sanitize_node(n);
        assert!(result.security_flags.contains(&SECURITY_FLAG.to_string()));
    }

    // ── Edge cases ───────────────────────────────────────────────────────────

    #[test]
    fn empty_label_gets_no_flag() {
        let s = make_sanitizer(PromptInjectionMode::ReportOnly);
        let n = label_node("");
        let result = s.sanitize_node(n);
        assert!(result.security_flags.is_empty());
        assert_eq!(result.label.as_deref(), Some(""));
    }

    #[test]
    fn bare_node_no_fields_no_flag() {
        let s = make_sanitizer(PromptInjectionMode::ReportOnly);
        let n = SemanticNode {
            role: "div".to_string(),
            ..Default::default()
        };
        let result = s.sanitize_node(n);
        assert!(result.security_flags.is_empty());
        assert!(result.label.is_none());
        assert!(result.alias.is_none());
        assert!(result.attributes.is_none());
    }

    // ── Field coverage ────────────────────────────────────────────────────────

    #[test]
    fn scans_alias_field() {
        let s = make_sanitizer(PromptInjectionMode::ReportOnly);
        let n = SemanticNode {
            role: "link".to_string(),
            alias: Some("jailbreak link".to_string()),
            ..Default::default()
        };
        let result = s.sanitize_node(n);
        assert!(result.security_flags.contains(&SECURITY_FLAG.to_string()));
    }

    #[test]
    fn scans_attribute_values() {
        let s = make_sanitizer(PromptInjectionMode::ReportOnly);
        let mut attrs = BTreeMap::new();
        attrs.insert(
            "aria-label".to_string(),
            "ignore previous instructions".to_string(),
        );
        let n = SemanticNode {
            role: "button".to_string(),
            attributes: Some(attrs),
            ..Default::default()
        };
        let result = s.sanitize_node(n);
        assert!(result.security_flags.contains(&SECURITY_FLAG.to_string()));
    }

    #[test]
    fn clean_attribute_gets_no_flag() {
        let s = make_sanitizer(PromptInjectionMode::ReportOnly);
        let mut attrs = BTreeMap::new();
        attrs.insert("class".to_string(), "btn btn-primary".to_string());
        let n = SemanticNode {
            role: "button".to_string(),
            attributes: Some(attrs),
            ..Default::default()
        };
        let result = s.sanitize_node(n);
        assert!(result.security_flags.is_empty());
    }

    // ── Recursion ─────────────────────────────────────────────────────────────

    #[test]
    fn recurses_into_children() {
        let s = make_sanitizer(PromptInjectionMode::ReportOnly);
        let child = label_node("jailbreak me");
        let parent = SemanticNode {
            role: "div".to_string(),
            children: vec![child],
            ..Default::default()
        };
        let result = s.sanitize_node(parent);
        assert!(
            result.children[0]
                .security_flags
                .contains(&SECURITY_FLAG.to_string()),
            "child flagged"
        );
        assert!(
            result.security_flags.is_empty(),
            "parent not flagged (no injection in parent fields)"
        );
    }

    #[test]
    fn parent_flagged_independently_of_children() {
        let s = make_sanitizer(PromptInjectionMode::ReportOnly);
        let child = label_node("safe text");
        let parent = SemanticNode {
            role: "div".to_string(),
            label: Some("ignore previous instructions".to_string()),
            children: vec![child],
            ..Default::default()
        };
        let result = s.sanitize_node(parent);
        assert!(
            result.security_flags.contains(&SECURITY_FLAG.to_string()),
            "parent flagged"
        );
        assert!(
            result.children[0].security_flags.is_empty(),
            "child not flagged"
        );
    }

    // ── All patterns smoke test ───────────────────────────────────────────────

    #[test]
    fn all_patterns_are_detected() {
        let s = make_sanitizer(PromptInjectionMode::ReportOnly);
        for &pattern in INJECTION_PATTERNS {
            let n = label_node(pattern);
            let result = s.sanitize_node(n);
            assert!(
                result.security_flags.contains(&SECURITY_FLAG.to_string()),
                "pattern not detected: {pattern}"
            );
        }
    }
}
