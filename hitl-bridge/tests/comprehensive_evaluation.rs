//! Comprehensive evaluation bench coverage for `hitl-bridge` (issue #190).
//!
//! Registers the two scenario families the crate is expected to cover in
//! `target/evaluation-dashboard.md`: the ACT-05 concurrent-approval lock race
//! and the prompt -> resolution notifier flow. Both mirror the fast, non-browser
//! scenarios already exercised in `tests/bridge_flow.rs`; the browser-backed
//! `PageSessionGateway` end-to-end test stays `#[ignore]`d there and is not
//! duplicated here (see docs/testing.md).

use std::sync::Arc;
use std::thread;

use core_runtime::{ApprovalScope, OutcomeProjection, RiskLevel};
use hitl_bridge::audit::BridgeAuditTrail;
use hitl_bridge::bridge::Bridge;
use hitl_bridge::gateway::mock::MockGateway;
use hitl_bridge::gateway::{ApprovalGateway, PendingApproval};
use hitl_bridge::lock::Decision;
use hitl_bridge::notifier::mock::MockNotifier;
use hitl_bridge::notifier::ChatNotifier;
use serde_json::{json, Value};
use test_bench_support::{EvaluationBench, EvaluationMode};
use uuid::Uuid;

fn sample_pending(id: Uuid) -> PendingApproval {
    PendingApproval {
        id,
        rule_id: "approve-pay".to_string(),
        action: "click".to_string(),
        target_signature: "sig-123".to_string(),
        scope: ApprovalScope::ActionOnly,
        outcome: Some(OutcomeProjection {
            projected_amount: Some(900.0),
            risk_level: RiskLevel::High,
        }),
    }
}

#[test]
fn test_hitl_bridge_comprehensive_evaluation_suite() -> anyhow::Result<()> {
    let mut bench = EvaluationBench::new(
        "hitl-bridge",
        "comprehensive_evaluation",
        EvaluationMode::from_env(),
    );

    bench.run_scenario(
        "concurrent_approval_lock_race",
        "hitl_concurrency",
        scenario_concurrent_approval_lock_race,
    );
    bench.run_scenario(
        "approval_prompt_and_resolution_flow",
        "hitl_notifier",
        scenario_approval_prompt_and_resolution_flow,
    );

    bench.write_if_configured()?;
    bench.assert_required_scenarios(&[
        "concurrent_approval_lock_race",
        "approval_prompt_and_resolution_flow",
    ])?;
    bench.assert_all_passed()?;
    Ok(())
}

/// Spec ACT-05: two reviewers racing to resolve the same request must apply
/// exactly one mutation to the gateway and write exactly one audit record.
fn scenario_concurrent_approval_lock_race() -> anyhow::Result<Value> {
    let dir = tempfile::tempdir()?;
    let id = Uuid::new_v4();

    let gateway = Arc::new(MockGateway::new(Some(sample_pending(id))));
    let notifier = Arc::new(MockNotifier::new());
    let audit = BridgeAuditTrail::new(dir.path().join("audit.ndjson"));
    let bridge = Arc::new(Bridge::new(
        gateway.clone() as Arc<dyn ApprovalGateway>,
        notifier.clone() as Arc<dyn ChatNotifier>,
        audit,
    ));

    bridge.poll_once()?;

    let decisions = [
        ("alice", Decision::Approved),
        ("bob", Decision::Rejected),
        ("carol", Decision::Approved),
    ];

    let handles: Vec<_> = decisions
        .into_iter()
        .map(|(name, decision)| {
            let bridge = Arc::clone(&bridge);
            thread::spawn(move || bridge.resolve(id, decision, name))
        })
        .collect();

    let results: Vec<anyhow::Result<()>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let successes = results.iter().filter(|r| r.is_ok()).count();
    let failures = results.iter().filter(|r| r.is_err()).count();

    if successes != 1 || failures != 2 {
        anyhow::bail!(
            "expected exactly one winner and two losers, got {successes} successes / {failures} failures"
        );
    }
    if gateway.resolutions().len() != 1 {
        anyhow::bail!("gateway must be mutated exactly once regardless of contention");
    }

    let records = BridgeAuditTrail::new(dir.path().join("audit.ndjson")).read_all()?;
    if records.len() != 1 {
        anyhow::bail!("expected exactly one audit record, got {}", records.len());
    }

    Ok(json!({
        "successes": successes,
        "failures": failures,
        "audit_records": records.len(),
    }))
}

/// A poll surfaces the pending request to the notifier, and resolving it
/// replaces the prompt with the final decision.
fn scenario_approval_prompt_and_resolution_flow() -> anyhow::Result<Value> {
    let dir = tempfile::tempdir()?;
    let id = Uuid::new_v4();

    let gateway = Arc::new(MockGateway::new(Some(sample_pending(id))));
    let notifier = Arc::new(MockNotifier::new());
    let audit = BridgeAuditTrail::new(dir.path().join("audit.ndjson"));
    let bridge = Bridge::new(
        gateway.clone() as Arc<dyn ApprovalGateway>,
        notifier.clone() as Arc<dyn ChatNotifier>,
        audit,
    );

    let notified = bridge.poll_once()?;
    if notified != Some(id) {
        anyhow::bail!("expected poll_once to notify about the pending request");
    }

    bridge.resolve(id, Decision::Approved, "alice")?;

    let calls = notifier.calls();
    if calls.len() != 2 {
        anyhow::bail!("expected exactly one notify followed by one respond, got {calls:?}");
    }
    if !matches!(calls[0], hitl_bridge::notifier::mock::Call::Notify(_)) {
        anyhow::bail!("first notifier call must be the prompt, got {:?}", calls[0]);
    }
    if !matches!(
        calls[1],
        hitl_bridge::notifier::mock::Call::Respond {
            decision: Decision::Approved,
            ..
        }
    ) {
        anyhow::bail!("second notifier call must be the resolution, got {:?}", calls[1]);
    }

    Ok(json!({
        "notified_id": notified.map(|id| id.to_string()),
        "notifier_calls": calls.len(),
    }))
}
