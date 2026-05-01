use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, path::Path};
use url::Url;

const DEFAULT_POLICY_RULES_JSON: &str = include_str!("../policy/default_rules.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    Allow,
    Block,
    RequireHumanApproval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ApprovalScope {
    ActionOnly,
    UntilNavigation,
    Timeboxed { ms: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyRule {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Path prefix to match against the URL path.
    ///
    /// Matching is segment-aware: `/login` matches `/login` and `/login/step`,
    /// but not `/logina`. Comparison is performed against the raw (percent-encoded)
    /// path returned by the URL parser — decoded forms (e.g. `/lo%67in`) will not
    /// match a rule for `/login`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_prefix: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_regex: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_regex: Option<String>,
    pub action: PolicyAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ApprovalScope>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyContext {
    pub url: String,
    pub action: String,
    pub target_role: Option<String>,
    pub target_text: Option<String>,
    pub surrounding_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyDecision {
    pub action: PolicyAction,
    pub rule_id: Option<String>,
    pub scope: Option<ApprovalScope>,
}

impl PolicyDecision {
    fn allow() -> Self {
        Self {
            action: PolicyAction::Allow,
            rule_id: None,
            scope: None,
        }
    }
}

#[derive(Debug, Clone)]
struct CompiledPolicyRule {
    raw: PolicyRule,
    text_regex: Option<Regex>,
    context_regex: Option<Regex>,
}

impl CompiledPolicyRule {
    fn try_new(raw: PolicyRule) -> Result<Self> {
        let text_regex = compile_regex(raw.text_regex.as_deref(), &raw.id, "text_regex")?;
        let context_regex = compile_regex(raw.context_regex.as_deref(), &raw.id, "context_regex")?;

        Ok(Self {
            raw,
            text_regex,
            context_regex,
        })
    }

    fn matches(&self, context: &NormalizedPolicyContext) -> bool {
        if let Some(domain) = self.raw.domain.as_deref() {
            let Some(actual_domain) = context.domain.as_deref() else {
                return false;
            };
            if !actual_domain.eq_ignore_ascii_case(domain.trim()) {
                return false;
            }
        }

        if let Some(path_prefix) = self.raw.path_prefix.as_deref() {
            let prefix = path_prefix.trim();
            if !context.path.starts_with(prefix) {
                return false;
            }
            // Segment-aware matching: the prefix must end at a segment boundary.
            // Accept if the path is exactly the prefix, or if the next character
            // after the prefix is '/', or if the prefix itself ends with '/'.
            let is_exact = context.path == prefix;
            let next_is_separator =
                context.path.as_bytes().get(prefix.len()).copied() == Some(b'/');
            let prefix_has_separator = prefix.ends_with('/');
            if !is_exact && !next_is_separator && !prefix_has_separator {
                return false;
            }
        }

        if let Some(role) = self.raw.role.as_deref() {
            let Some(actual_role) = context.target_role.as_deref() else {
                return false;
            };
            if !actual_role.eq_ignore_ascii_case(role.trim()) {
                return false;
            }
        }

        if let Some(regex) = &self.text_regex {
            let Some(target_text) = context.target_text.as_deref() else {
                return false;
            };
            if !regex.is_match(target_text) {
                return false;
            }
        }

        if let Some(regex) = &self.context_regex {
            let Some(surrounding_text) = context.surrounding_text.as_deref() else {
                return false;
            };
            if !regex.is_match(surrounding_text) {
                return false;
            }
        }

        true
    }

    fn to_decision(&self) -> PolicyDecision {
        let scope = match self.raw.action {
            PolicyAction::RequireHumanApproval => {
                Some(self.raw.scope.unwrap_or(ApprovalScope::ActionOnly))
            }
            _ => None,
        };

        PolicyDecision {
            action: self.raw.action,
            rule_id: Some(self.raw.id.clone()),
            scope,
        }
    }
}

#[derive(Debug, Clone)]
struct NormalizedPolicyContext {
    domain: Option<String>,
    path: String,
    target_role: Option<String>,
    target_text: Option<String>,
    surrounding_text: Option<String>,
}

impl NormalizedPolicyContext {
    fn from_input(input: &PolicyContext) -> Self {
        let parsed_url = Url::parse(&input.url).ok();
        let domain = parsed_url
            .as_ref()
            .and_then(|url| url.host_str())
            .map(|host| host.to_lowercase());
        let path = parsed_url
            .as_ref()
            .map(|url| url.path().to_string())
            .unwrap_or_else(|| "/".to_string());

        Self {
            domain,
            path,
            target_role: input.target_role.as_deref().map(normalize_optional_text),
            target_text: input.target_text.as_deref().map(normalize_optional_text),
            surrounding_text: input
                .surrounding_text
                .as_deref()
                .map(normalize_optional_text),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PolicyEngine {
    rules: Vec<PolicyRule>,
    compiled_rules: Vec<CompiledPolicyRule>,
}

impl Default for PolicyEngine {
    /// Loads the embedded default policy rules.
    ///
    /// # Panics
    /// Panics if the embedded `default_rules.json` asset is malformed.
    /// This is intentional: a broken build-time asset must be caught at
    /// startup rather than silently falling back to an allow-all policy
    /// (fail-closed, not fail-open).
    fn default() -> Self {
        Self::try_from_json_str(DEFAULT_POLICY_RULES_JSON)
            .expect("Embedded default policy rules are malformed — this is a build-time bug")
    }
}

impl PolicyEngine {
    pub fn empty() -> Self {
        Self {
            rules: Vec::new(),
            compiled_rules: Vec::new(),
        }
    }

    pub fn try_new(rules: Vec<PolicyRule>) -> Result<Self> {
        validate_rule_set(&rules)?;
        let compiled_rules = rules
            .iter()
            .cloned()
            .map(CompiledPolicyRule::try_new)
            .collect::<Result<Vec<_>>>()?;

        Ok(Self {
            rules,
            compiled_rules,
        })
    }

    pub fn try_from_json_str(json: &str) -> Result<Self> {
        let rules: Vec<PolicyRule> =
            serde_json::from_str(json).context("Failed to deserialize policy rules JSON")?;
        Self::try_new(rules)
    }

    pub fn try_from_file(path: impl AsRef<Path>) -> Result<Self> {
        let path_ref = path.as_ref();
        let body = fs::read_to_string(path_ref)
            .with_context(|| format!("Failed to read policy rule file: {}", path_ref.display()))?;
        Self::try_from_json_str(&body)
            .with_context(|| format!("Failed to parse policy rule file: {}", path_ref.display()))
    }

    pub fn evaluate(&self, context: &PolicyContext) -> PolicyDecision {
        let normalized = NormalizedPolicyContext::from_input(context);
        for rule in &self.compiled_rules {
            if rule.matches(&normalized) {
                return rule.to_decision();
            }
        }
        PolicyDecision::allow()
    }

    pub fn rules(&self) -> &[PolicyRule] {
        &self.rules
    }
}

pub fn validate_rule_set(rules: &[PolicyRule]) -> Result<()> {
    let mut seen_ids = HashSet::with_capacity(rules.len());
    for rule in rules {
        let id = rule.id.trim();
        anyhow::ensure!(!id.is_empty(), "Policy rule id must not be empty");
        anyhow::ensure!(
            seen_ids.insert(id.to_string()),
            "Duplicated policy rule id: {id}"
        );

        // A whitespace-only path_prefix trims to "", making starts_with("") always
        // true — effectively a wildcard that bypasses all subsequent rules.
        if let Some(prefix) = rule.path_prefix.as_deref() {
            anyhow::ensure!(
                !prefix.trim().is_empty(),
                "Policy rule '{}' has an empty or whitespace-only path_prefix (would match all paths)",
                rule.id
            );
        }

        match rule.action {
            PolicyAction::RequireHumanApproval => {
                anyhow::ensure!(
                    rule.scope.is_some(),
                    "Policy rule '{}' requires a scope for require_human_approval",
                    rule.id
                );
            }
            PolicyAction::Allow | PolicyAction::Block => {
                anyhow::ensure!(
                    rule.scope.is_none(),
                    "Policy rule '{}' sets scope but action is not require_human_approval",
                    rule.id
                );
            }
        }

        compile_regex(rule.text_regex.as_deref(), &rule.id, "text_regex")?;
        compile_regex(rule.context_regex.as_deref(), &rule.id, "context_regex")?;
    }

    Ok(())
}

fn compile_regex(pattern: Option<&str>, rule_id: &str, field_name: &str) -> Result<Option<Regex>> {
    let Some(pattern) = pattern else {
        return Ok(None);
    };

    let trimmed = pattern.trim();
    anyhow::ensure!(
        !trimmed.is_empty(),
        "Policy rule '{rule_id}' has empty {field_name}"
    );

    let compiled = Regex::new(trimmed).with_context(|| {
        format!("Policy rule '{rule_id}' has invalid regex in {field_name}: {trimmed}")
    })?;
    Ok(Some(compiled))
}

fn normalize_optional_text(input: &str) -> String {
    input.trim().to_lowercase()
}
