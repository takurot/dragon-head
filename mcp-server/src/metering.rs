use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Default)]
#[serde(rename_all = "snake_case")]
pub enum PlanTier {
    Developer,
    Pro,
    #[default]
    Enterprise,
}

impl PlanTier {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            PlanTier::Developer => "developer",
            PlanTier::Pro => "pro",
            PlanTier::Enterprise => "enterprise",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AuditRetentionSnapshot {
    pub retained_events: u64,
    pub retained_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct StateGenerationUsage {
    pub fast: u64,
    pub full: u64,
    pub delta: u64,
    /// Number of `get_state` calls served from a verified speculative
    /// pre-generation (near-zero TTFT hit, Spec §3.5 / ISSUE-147).
    #[serde(default)]
    pub speculative: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UsageReport {
    pub plan_tier: PlanTier,
    pub state_generations: StateGenerationUsage,
    pub visual_captures: u64,
    pub actions_executed: u64,
    pub hitl_events: u64,
    pub audit_retention: AuditRetentionSnapshot,
    pub cost_microusd: UsageCostBreakdown,
    /// Number of times the underlying Chrome process was automatically
    /// restarted after a crash/disconnect (ISSUE-149).
    pub browser_restarts: u64,
    /// Number of `get_state` calls where a speculative prediction was
    /// attempted but missed, falling back to a full state capture
    /// (ISSUE-147).
    #[serde(default)]
    pub speculative_misses: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UsageCostBreakdown {
    pub state_generations: u64,
    pub visual_captures: u64,
    pub actions_executed: u64,
    pub hitl_events: u64,
    pub audit_retention: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct UsageMeters {
    pub(crate) state_generations: StateGenerationUsage,
    pub(crate) visual_captures: u64,
    pub(crate) actions_executed: u64,
    /// HITL events are counted **twice** per resolved interaction: once when `act` returns
    /// `requires_human_approval` (the trigger) and once when `ask_human` returns `approved=true`
    /// (the resolution). This is intentional — both sides of the HITL interaction represent
    /// distinct, metered value-based events per Section 7.1 of the billing spec.
    pub(crate) hitl_events: u64,
}

impl UsageMeters {
    pub(crate) fn to_report(
        &self,
        plan_tier: PlanTier,
        audit_retention: AuditRetentionSnapshot,
        browser_restarts: u64,
        speculative_misses: u64,
    ) -> UsageReport {
        let cost_microusd = estimate_usage_cost(
            plan_tier,
            &self.state_generations,
            self.visual_captures,
            self.actions_executed,
            self.hitl_events,
            audit_retention,
        );

        UsageReport {
            plan_tier,
            state_generations: self.state_generations.clone(),
            visual_captures: self.visual_captures,
            actions_executed: self.actions_executed,
            hitl_events: self.hitl_events,
            audit_retention,
            cost_microusd,
            browser_restarts,
            speculative_misses,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanFeature {
    SemanticDelta,
    SomVisualCapture,
    PolicyHumanApproval,
}

impl PlanFeature {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            PlanFeature::SemanticDelta => "semantic_delta",
            PlanFeature::SomVisualCapture => "som_visual_capture",
            PlanFeature::PolicyHumanApproval => "policy_human_approval",
        }
    }

    pub(crate) fn required_plan(self) -> PlanTier {
        match self {
            PlanFeature::SemanticDelta => PlanTier::Pro,
            PlanFeature::SomVisualCapture => PlanTier::Pro,
            PlanFeature::PolicyHumanApproval => PlanTier::Enterprise,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PricingRateCard {
    state_fast: u64,
    state_full: u64,
    state_delta: u64,
    /// Rate per speculative (cache-hit) state generation. Kept below
    /// `state_fast` to reflect the near-zero marginal cost of serving a
    /// pre-generated snapshot (Spec §3.5 / ISSUE-147).
    state_speculative: u64,
    visual_capture: u64,
    action_executed: u64,
    hitl_event: u64,
    audit_retention_per_mib: u64,
}

fn pricing_rate_card(plan_tier: PlanTier) -> PricingRateCard {
    match plan_tier {
        PlanTier::Developer => PricingRateCard {
            state_fast: 0,
            state_full: 0,
            state_delta: 0,
            state_speculative: 0,
            visual_capture: 0,
            action_executed: 0,
            hitl_event: 0,
            audit_retention_per_mib: 0,
        },
        PlanTier::Pro => PricingRateCard {
            state_fast: 100,
            state_full: 250,
            state_delta: 50,
            state_speculative: 20,
            visual_capture: 1_000,
            action_executed: 75,
            hitl_event: 1_500,
            audit_retention_per_mib: 400,
        },
        PlanTier::Enterprise => PricingRateCard {
            state_fast: 80,
            state_full: 200,
            state_delta: 40,
            state_speculative: 15,
            visual_capture: 850,
            action_executed: 60,
            hitl_event: 1_200,
            audit_retention_per_mib: 300,
        },
    }
}

pub fn estimate_usage_cost(
    plan_tier: PlanTier,
    state_generations: &StateGenerationUsage,
    visual_captures: u64,
    actions_executed: u64,
    hitl_events: u64,
    audit_retention: AuditRetentionSnapshot,
) -> UsageCostBreakdown {
    const MIB: u64 = 1_048_576;

    let rates = pricing_rate_card(plan_tier);
    let state_generations_cost = state_generations
        .fast
        .saturating_mul(rates.state_fast)
        .saturating_add(state_generations.full.saturating_mul(rates.state_full))
        .saturating_add(state_generations.delta.saturating_mul(rates.state_delta))
        .saturating_add(
            state_generations
                .speculative
                .saturating_mul(rates.state_speculative),
        );
    let visual_captures_cost = visual_captures.saturating_mul(rates.visual_capture);
    let actions_executed_cost = actions_executed.saturating_mul(rates.action_executed);
    let hitl_events_cost = hitl_events.saturating_mul(rates.hitl_event);
    let retained_mib = audit_retention.retained_bytes.saturating_add(MIB - 1) / MIB;
    let audit_retention_cost = retained_mib.saturating_mul(rates.audit_retention_per_mib);
    let total = state_generations_cost
        .saturating_add(visual_captures_cost)
        .saturating_add(actions_executed_cost)
        .saturating_add(hitl_events_cost)
        .saturating_add(audit_retention_cost);

    UsageCostBreakdown {
        state_generations: state_generations_cost,
        visual_captures: visual_captures_cost,
        actions_executed: actions_executed_cost,
        hitl_events: hitl_events_cost,
        audit_retention: audit_retention_cost,
        total,
    }
}

/// Metered operations accumulated during a single `run_skill` execution.
///
/// `McpBackend::take_skill_usage_delta` returns this after each `run_skill` call so
/// `McpServer` can fold the counts into its top-level `UsageMeters`.
///
/// Counts are per-invocation of `run_skill`. `McpServer` merges them additively into the
/// session-level meters after every successful or partially-failed skill run.
#[derive(Debug, Clone, Default)]
pub struct SkillUsageDelta {
    /// Number of `act` steps that completed successfully inside the skill.
    pub actions_executed: u64,
    /// Number of visual-capture steps that completed inside the skill (reserved; not yet
    /// incremented by `PageSkillRuntime` — no `get_visual` step type exists in the skill DSL).
    pub visual_captures: u64,
    /// Number of HITL events triggered inside the skill (reserved; not yet incremented by
    /// `PageSkillRuntime` — skills do not have `ask_human` steps).
    pub hitl_events: u64,
}
