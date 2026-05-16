use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, path::Path};
use url::Url;

const DEFAULT_POLICY_RULES_JSON: &str = include_str!("../policy/default_rules.json");

// ─── Guardian Angel: Outcome Projection types ────────────────────────────────

/// Risk classification derived from outcome projection threshold comparisons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    /// No threshold exceeded; no monetary amount detected.
    Low,
    /// Monetary amount detected but no configured threshold exceeded.
    Medium,
    /// Warn-threshold exceeded; human approval recommended.
    High,
    /// Block-threshold exceeded; execution prevented.
    Critical,
}

/// Structured prediction of an action's side effects, generated before execution.
///
/// Attached to [`PolicyDecision`] and surfaced in
/// [`ActionError::HumanApprovalRequired`] so callers can display the projected
/// impact to a human before the action is approved or rejected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutcomeProjection {
    /// Projected monetary amount (USD or unspecified currency) extracted from
    /// surrounding page context, if the `amount_regex` captured a value.
    pub projected_amount: Option<f64>,
    /// Risk classification based on configured thresholds.
    pub risk_level: RiskLevel,
}

/// Configuration for the Guardian Angel outcome projector attached to a rule.
///
/// When a rule matches, the projector extracts side-effect data from the action
/// context and can proactively upgrade the policy action (Allow → Block / HITL)
/// if configured thresholds are exceeded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeProjectorConfig {
    /// Regex with a named capture group `amount` to extract a monetary value
    /// (digits and optional comma-separators / decimal point) from
    /// `surrounding_text`.  Example: `r"\$\s*(?P<amount>[\d,]+(?:\.\d{1,2})?)"`.
    ///
    /// **Note:** `surrounding_text` is pre-normalized (trimmed and lowercased)
    /// before matching.  Write case-insensitive patterns or lower-case literals
    /// accordingly (e.g. `\$` works fine; `USD` must be written as `usd`).
    ///
    /// When multiple matches exist the **largest** extracted value is used so
    /// that a page cannot suppress a block threshold by prepending a smaller
    /// amount mention (e.g. "save $10 off your $900 total").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount_regex: Option<String>,
    /// If the extracted amount exceeds this value the policy action is
    /// **proactively upgraded to `Block`** regardless of the rule's declared
    /// action.  Produces `RiskLevel::Critical`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_if_amount_exceeds: Option<f64>,
    /// If the extracted amount exceeds this value (but not `block_if_amount_exceeds`)
    /// the policy action is upgraded to `RequireHumanApproval` if it is currently
    /// `Allow`.  Produces `RiskLevel::High`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warn_if_amount_exceeds: Option<f64>,
}

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

// PartialEq only (not Eq): OutcomeProjectorConfig contains f64 fields which are
// not totally ordered and therefore cannot implement Eq.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// Guardian Angel: optional outcome projector configuration.
    ///
    /// When set, the engine extracts side-effect data from the action context
    /// (e.g. payment amount) and can proactively upgrade the action to Block or
    /// RequireHumanApproval if configured thresholds are exceeded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome_projector: Option<OutcomeProjectorConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyContext {
    pub url: String,
    pub action: String,
    pub target_role: Option<String>,
    pub target_text: Option<String>,
    pub surrounding_text: Option<String>,
}

// PartialEq only (not Eq): OutcomeProjection contains f64 (projected_amount).
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyDecision {
    pub action: PolicyAction,
    pub rule_id: Option<String>,
    pub scope: Option<ApprovalScope>,
    /// Guardian Angel: structured outcome projection for this action, if computed.
    pub outcome: Option<OutcomeProjection>,
}

impl PolicyDecision {
    fn allow() -> Self {
        Self {
            action: PolicyAction::Allow,
            rule_id: None,
            scope: None,
            outcome: None,
        }
    }
}

#[derive(Debug, Clone)]
struct CompiledPolicyRule {
    raw: PolicyRule,
    text_regex: Option<Regex>,
    context_regex: Option<Regex>,
    /// Compiled amount extractor from `outcome_projector.amount_regex`.
    amount_regex: Option<Regex>,
}

impl CompiledPolicyRule {
    fn try_new(raw: PolicyRule) -> Result<Self> {
        let text_regex = compile_regex(raw.text_regex.as_deref(), &raw.id, "text_regex")?;
        let context_regex = compile_regex(raw.context_regex.as_deref(), &raw.id, "context_regex")?;
        let amount_regex = raw
            .outcome_projector
            .as_ref()
            .and_then(|p| p.amount_regex.as_deref())
            .map(|pat| {
                compile_regex(Some(pat), &raw.id, "outcome_projector.amount_regex")
                    .and_then(|r| r.ok_or_else(|| anyhow::anyhow!("empty amount_regex")))
            })
            .transpose()?;

        Ok(Self {
            raw,
            text_regex,
            context_regex,
            amount_regex,
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

    fn to_decision(&self, context: &NormalizedPolicyContext) -> PolicyDecision {
        let (action, outcome) = self.compute_outcome(context);

        let scope = match action {
            PolicyAction::RequireHumanApproval => {
                Some(self.raw.scope.unwrap_or(ApprovalScope::ActionOnly))
            }
            _ => None,
        };

        PolicyDecision {
            action,
            rule_id: Some(self.raw.id.clone()),
            scope,
            outcome,
        }
    }

    /// Extract outcome projection from context and potentially upgrade the action.
    ///
    /// Returns `(final_action, outcome)` where `final_action` may be promoted
    /// from the rule's declared action when a threshold is exceeded.
    fn compute_outcome(
        &self,
        context: &NormalizedPolicyContext,
    ) -> (PolicyAction, Option<OutcomeProjection>) {
        let Some(projector) = self.raw.outcome_projector.as_ref() else {
            return (self.raw.action, None);
        };

        // Extract the LARGEST monetary amount from surrounding_text.
        //
        // Using the maximum across all matches is the conservative (safe) choice:
        // a malicious page that prepends a small amount (e.g. "save $10 off your
        // $900 total") cannot suppress a block threshold by being seen first.
        let projected_amount = self
            .amount_regex
            .as_ref()
            .zip(context.surrounding_text.as_deref())
            .and_then(|(re, text)| {
                re.captures_iter(text)
                    .filter_map(|caps| {
                        let raw = caps.name("amount")?.as_str();
                        // Strip comma separators before parsing ("1,234.56" → 1234.56)
                        raw.replace(',', "").parse::<f64>().ok()
                    })
                    .reduce(f64::max)
            });

        let (action, risk_level) = match projected_amount {
            Some(amount) => {
                if projector
                    .block_if_amount_exceeds
                    .is_some_and(|threshold| amount > threshold)
                {
                    (PolicyAction::Block, RiskLevel::Critical)
                } else if projector
                    .warn_if_amount_exceeds
                    .is_some_and(|threshold| amount > threshold)
                {
                    // Promote Allow → HITL; Block stays Block.
                    let promoted = match self.raw.action {
                        PolicyAction::Allow => PolicyAction::RequireHumanApproval,
                        other => other,
                    };
                    (promoted, RiskLevel::High)
                } else {
                    (self.raw.action, RiskLevel::Medium)
                }
            }
            None => (self.raw.action, RiskLevel::Low),
        };

        (
            action,
            Some(OutcomeProjection {
                projected_amount,
                risk_level,
            }),
        )
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
                return rule.to_decision(&normalized);
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

        // Guardian Angel outcome projector validations.
        if let Some(projector) = &rule.outcome_projector {
            // A threshold without an amount_regex is a silent no-op.
            if (projector.block_if_amount_exceeds.is_some()
                || projector.warn_if_amount_exceeds.is_some())
                && projector.amount_regex.is_none()
            {
                anyhow::bail!(
                    "Policy rule '{}': outcome_projector sets a threshold but has no \
                     amount_regex to extract amounts from",
                    rule.id
                );
            }

            // block threshold must be >= warn threshold; otherwise the warn branch
            // is unreachable dead configuration.
            if let (Some(block), Some(warn)) = (
                projector.block_if_amount_exceeds,
                projector.warn_if_amount_exceeds,
            ) {
                anyhow::ensure!(
                    block >= warn,
                    "Policy rule '{}': block_if_amount_exceeds ({}) must be >= \
                     warn_if_amount_exceeds ({})",
                    rule.id,
                    block,
                    warn
                );
            }

            compile_regex(
                projector.amount_regex.as_deref(),
                &rule.id,
                "outcome_projector.amount_regex",
            )?;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn run_policy(rules: Vec<PolicyRule>, surrounding_text: Option<&str>) -> PolicyDecision {
        let engine = PolicyEngine::try_new(rules).expect("valid rules");
        engine.evaluate(&PolicyContext {
            url: "https://shop.example.com/checkout".to_string(),
            action: "click".to_string(),
            target_role: Some("button".to_string()),
            target_text: Some("Pay Now".to_string()),
            surrounding_text: surrounding_text.map(|s| s.to_string()),
        })
    }

    fn pay_rule(projector: OutcomeProjectorConfig) -> PolicyRule {
        PolicyRule {
            id: "guardian-pay".to_string(),
            domain: None,
            path_prefix: None,
            role: None,
            text_regex: None,
            context_regex: None,
            action: PolicyAction::RequireHumanApproval,
            scope: Some(ApprovalScope::ActionOnly),
            outcome_projector: Some(projector),
        }
    }

    fn amount_projector(block: Option<f64>, warn: Option<f64>) -> OutcomeProjectorConfig {
        OutcomeProjectorConfig {
            amount_regex: Some(r"\$\s*(?P<amount>[\d,]+(?:\.\d{1,2})?)".to_string()),
            block_if_amount_exceeds: block,
            warn_if_amount_exceeds: warn,
        }
    }

    #[test]
    fn outcome_projector_extracts_amount_from_surrounding_text() {
        let rule = pay_rule(amount_projector(None, None));
        let decision = run_policy(vec![rule], Some("Total: $250.00"));
        let proj = decision.outcome.expect("outcome must be present");
        assert_eq!(proj.projected_amount, Some(250.0));
        assert_eq!(proj.risk_level, RiskLevel::Medium);
    }

    #[test]
    fn outcome_projector_handles_comma_separated_amounts() {
        let rule = pay_rule(amount_projector(None, None));
        let decision = run_policy(vec![rule], Some("Order total: $1,234.56"));
        let proj = decision.outcome.unwrap();
        assert_eq!(proj.projected_amount, Some(1234.56));
    }

    #[test]
    fn proactive_block_when_amount_exceeds_block_threshold() {
        let rule = pay_rule(amount_projector(Some(500.0), Some(100.0)));
        let decision = run_policy(vec![rule], Some("Grand total $750"));
        assert_eq!(decision.action, PolicyAction::Block);
        let proj = decision.outcome.unwrap();
        assert_eq!(proj.risk_level, RiskLevel::Critical);
        assert_eq!(proj.projected_amount, Some(750.0));
    }

    #[test]
    fn proactive_hitl_when_amount_exceeds_warn_threshold_only() {
        let rule = pay_rule(amount_projector(Some(500.0), Some(100.0)));
        let decision = run_policy(vec![rule], Some("Total: $300"));
        assert_eq!(decision.action, PolicyAction::RequireHumanApproval);
        let proj = decision.outcome.unwrap();
        assert_eq!(proj.risk_level, RiskLevel::High);
    }

    #[test]
    fn allow_rule_promoted_to_hitl_when_warn_threshold_exceeded() {
        let rule = PolicyRule {
            id: "guardian-allow-promoted".to_string(),
            domain: None,
            path_prefix: None,
            role: None,
            text_regex: None,
            context_regex: None,
            action: PolicyAction::Allow,
            scope: None,
            outcome_projector: Some(amount_projector(Some(1000.0), Some(50.0))),
        };
        let decision = run_policy(vec![rule], Some("Amount: $75.00"));
        assert_eq!(decision.action, PolicyAction::RequireHumanApproval);
        let proj = decision.outcome.unwrap();
        assert_eq!(proj.risk_level, RiskLevel::High);
    }

    #[test]
    fn no_upgrade_when_amount_below_both_thresholds() {
        let rule = pay_rule(amount_projector(Some(500.0), Some(100.0)));
        let decision = run_policy(vec![rule], Some("Subtotal: $50.00"));
        assert_eq!(decision.action, PolicyAction::RequireHumanApproval);
        let proj = decision.outcome.unwrap();
        assert_eq!(proj.risk_level, RiskLevel::Medium);
        assert_eq!(proj.projected_amount, Some(50.0));
    }

    #[test]
    fn low_risk_when_no_surrounding_text() {
        let rule = pay_rule(amount_projector(Some(500.0), Some(100.0)));
        let decision = run_policy(vec![rule], None);
        let proj = decision.outcome.unwrap();
        assert_eq!(proj.projected_amount, None);
        assert_eq!(proj.risk_level, RiskLevel::Low);
        assert_eq!(decision.action, PolicyAction::RequireHumanApproval);
    }

    #[test]
    fn no_outcome_when_projector_not_configured() {
        let rule = PolicyRule {
            id: "plain-block".to_string(),
            domain: None,
            path_prefix: None,
            role: None,
            text_regex: None,
            context_regex: None,
            action: PolicyAction::Block,
            scope: None,
            outcome_projector: None,
        };
        let decision = run_policy(vec![rule], Some("Total: $999.00"));
        assert_eq!(decision.outcome, None);
    }

    #[test]
    fn policy_rule_with_projector_roundtrips_json() {
        let rule = pay_rule(amount_projector(Some(1000.0), Some(200.0)));
        let json = serde_json::to_string(&rule).expect("serialize");
        let restored: PolicyRule = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(rule, restored);
    }

    #[test]
    fn legacy_rules_without_projector_load_correctly() {
        let json = r#"[{
            "id": "legacy-block",
            "role": "button",
            "text_regex": "delete",
            "action": "block"
        }]"#;
        let engine = PolicyEngine::try_from_json_str(json).expect("legacy rules must load");
        let decision = engine.evaluate(&PolicyContext {
            url: "https://example.com".to_string(),
            action: "click".to_string(),
            target_role: Some("button".to_string()),
            target_text: Some("Delete".to_string()),
            surrounding_text: None,
        });
        assert_eq!(decision.action, PolicyAction::Block);
        assert_eq!(decision.outcome, None);
    }

    #[test]
    fn largest_amount_used_when_multiple_dollar_signs_in_text() {
        // "save $10 off your total of $900" — must use $900, not $10.
        let rule = pay_rule(amount_projector(Some(500.0), Some(100.0)));
        let decision = run_policy(vec![rule], Some("save $10 off your total of $900"));
        assert_eq!(
            decision.action,
            PolicyAction::Block,
            "must block on largest amount $900"
        );
        let proj = decision.outcome.unwrap();
        assert_eq!(proj.projected_amount, Some(900.0));
        assert_eq!(proj.risk_level, RiskLevel::Critical);
    }

    #[test]
    fn low_risk_when_regex_present_but_no_match_in_text() {
        // surrounding_text exists but contains no dollar amount.
        let rule = pay_rule(amount_projector(Some(500.0), Some(100.0)));
        let decision = run_policy(vec![rule], Some("No amount mentioned here"));
        let proj = decision.outcome.unwrap();
        assert_eq!(proj.projected_amount, None);
        assert_eq!(proj.risk_level, RiskLevel::Low);
        assert_eq!(decision.action, PolicyAction::RequireHumanApproval);
    }

    #[test]
    fn warn_only_threshold_promotes_allow_to_hitl() {
        // Only warn threshold configured (no block threshold).
        let rule = PolicyRule {
            id: "warn-only-gate".to_string(),
            domain: None,
            path_prefix: None,
            role: None,
            text_regex: None,
            context_regex: None,
            action: PolicyAction::Allow,
            scope: None,
            outcome_projector: Some(OutcomeProjectorConfig {
                amount_regex: Some(r"\$\s*(?P<amount>[\d,]+(?:\.\d{1,2})?)".to_string()),
                block_if_amount_exceeds: None,
                warn_if_amount_exceeds: Some(50.0),
            }),
        };
        let decision = run_policy(vec![rule], Some("Fee: $75.00"));
        assert_eq!(decision.action, PolicyAction::RequireHumanApproval);
        let proj = decision.outcome.unwrap();
        assert_eq!(proj.risk_level, RiskLevel::High);
        assert_eq!(proj.projected_amount, Some(75.0));
        // When promoted from Allow, scope defaults to ActionOnly.
        assert_eq!(decision.scope, Some(ApprovalScope::ActionOnly));
    }

    #[test]
    fn validate_rejects_threshold_without_amount_regex() {
        let rule = PolicyRule {
            id: "bad-projector".to_string(),
            domain: None,
            path_prefix: None,
            role: None,
            text_regex: None,
            context_regex: None,
            action: PolicyAction::Block,
            scope: None,
            outcome_projector: Some(OutcomeProjectorConfig {
                amount_regex: None,
                block_if_amount_exceeds: Some(500.0),
                warn_if_amount_exceeds: None,
            }),
        };
        let err = PolicyEngine::try_new(vec![rule]).unwrap_err();
        assert!(
            err.to_string().contains("amount_regex"),
            "error must mention amount_regex: {err}"
        );
    }

    #[test]
    fn validate_rejects_inverted_thresholds() {
        let rule = pay_rule(OutcomeProjectorConfig {
            amount_regex: Some(r"\$\s*(?P<amount>[\d]+)".to_string()),
            block_if_amount_exceeds: Some(100.0),
            warn_if_amount_exceeds: Some(500.0), // warn > block — invalid
        });
        let err = PolicyEngine::try_new(vec![rule]).unwrap_err();
        assert!(
            err.to_string().contains("block_if_amount_exceeds"),
            "error must mention threshold ordering: {err}"
        );
    }
}
