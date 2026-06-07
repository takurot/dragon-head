//! Integration coverage for the HITL bridge orchestration.
//!
//! The fast suite (`cargo test -p hitl-bridge`) exercises the full
//! gateway → lock → audit → notifier pipeline against `MockGateway` /
//! `MockNotifier` — including the concurrency scenario from spec ACT-05
//! ("複数人による同時承認リクエストの競合回避検証": concurrent approvals of
//! the same request must not double-apply). One `#[ignore]`d test wires a
//! real `BrowserClient` end to end; run it explicitly with a live Chrome via
//! `cargo test -p hitl-bridge -- --include-ignored`.

use std::sync::Arc;
use std::thread;

use core_runtime::{ApprovalScope, OutcomeProjection, RiskLevel};
use hitl_bridge::audit::{AuditDecision, BridgeAuditTrail};
use hitl_bridge::bridge::Bridge;
use hitl_bridge::gateway::mock::MockGateway;
use hitl_bridge::gateway::{ApprovalGateway, PendingApproval};
use hitl_bridge::lock::Decision;
use hitl_bridge::notifier::mock::MockNotifier;
use hitl_bridge::notifier::ChatNotifier;
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

/// Two reviewers race to resolve the same request — exactly one mutation must
/// land on the gateway, exactly one audit record must be written, and the
/// loser must come back with an error rather than silently no-op'ing.
#[test]
fn concurrent_resolutions_of_the_same_request_apply_exactly_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let id = Uuid::new_v4();

    let gateway = Arc::new(MockGateway::new(Some(sample_pending(id))));
    let notifier = Arc::new(MockNotifier::new());
    let audit = BridgeAuditTrail::new(dir.path().join("audit.ndjson"));
    let bridge = Arc::new(Bridge::new(
        gateway.clone() as Arc<dyn ApprovalGateway>,
        notifier.clone() as Arc<dyn ChatNotifier>,
        audit,
    ));

    bridge.poll_once().expect("initial poll should notify");

    let decisions = [
        ("alice", Decision::Approved),
        ("bob", Decision::Rejected),
        ("carol", Decision::Approved),
        ("dave", Decision::Rejected),
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

    assert_eq!(successes, 1, "exactly one resolution attempt must succeed");
    assert_eq!(
        failures, 3,
        "the rest must lose the lock and report an error"
    );

    assert_eq!(
        gateway.resolutions().len(),
        1,
        "the gateway must be mutated exactly once regardless of contention"
    );

    let records = BridgeAuditTrail::new(dir.path().join("audit.ndjson"))
        .read_all()
        .expect("read audit trail");
    assert_eq!(records.len(), 1, "exactly one audit record must be written");
    assert!(
        matches!(
            records[0].decision,
            AuditDecision::Approved | AuditDecision::Rejected
        ),
        "the single audit record must reflect the winning decision"
    );
    assert!(
        decisions
            .iter()
            .any(|(name, _)| *name == records[0].decided_by),
        "the audit record must attribute the decision to one of the contending reviewers"
    );
    assert!(
        records[0].outcome_projection.is_some(),
        "ACT-05 requires the audit record to carry Outcome Projection data"
    );
}

#[test]
fn poll_then_resolve_drives_notifier_through_prompt_and_resolution() {
    let dir = tempfile::tempdir().expect("tempdir");
    let id = Uuid::new_v4();

    let gateway = Arc::new(MockGateway::new(Some(sample_pending(id))));
    let notifier = Arc::new(MockNotifier::new());
    let audit = BridgeAuditTrail::new(dir.path().join("audit.ndjson"));
    let bridge = Bridge::new(
        gateway.clone() as Arc<dyn ApprovalGateway>,
        notifier.clone() as Arc<dyn ChatNotifier>,
        audit,
    );

    let notified = bridge.poll_once().expect("poll should succeed");
    assert_eq!(notified, Some(id));

    bridge
        .resolve(id, Decision::Approved, "alice")
        .expect("resolve should succeed");

    let calls = notifier.calls();
    assert_eq!(calls.len(), 2, "exactly one notify followed by one respond");
    assert!(matches!(
        calls[0],
        hitl_bridge::notifier::mock::Call::Notify(_)
    ));
    assert!(matches!(
        calls[1],
        hitl_bridge::notifier::mock::Call::Respond {
            decision: Decision::Approved,
            ..
        }
    ));
}

/// End-to-end wiring against a live session: raises a real `HumanApprovalRequired`,
/// resolves it through `PageSessionGateway`, and confirms the action stays
/// blocked until approved. Mirrors `bench/`'s `#[ignore]` browser test pattern —
/// run explicitly with `cargo test -p hitl-bridge -- --include-ignored`.
#[test]
#[ignore]
fn page_session_gateway_resolves_a_real_pending_approval() -> anyhow::Result<()> {
    use core_runtime::sre::{normalize_dom, LoadProfile, SemanticState};
    use core_runtime::{ActionError, BrowserClient, PolicyAction, PolicyRule};
    use hitl_bridge::gateway::PageSessionGateway;

    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = Arc::new(client.new_page()?);
    page.set_policy_rules(vec![PolicyRule {
        id: "approve-pay".to_string(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_string()),
        text_regex: Some("(?i)pay".to_string()),
        context_regex: None,
        action: PolicyAction::RequireHumanApproval,
        scope: Some(ApprovalScope::ActionOnly),
        outcome_projector: None,
    }])?;

    let html = r#"
        <html>
            <body>
                <button id="pay" onclick="document.body.dataset.paid = 'true'">Pay</button>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let root = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Interactive, &root)?;
    let state = SemanticState::new(sem, LoadProfile::Interactive);
    let (target_id, target_key) = find_button(state.root()).expect("button must be present");

    let first_attempt = page.act(Some(target_id), Some(&target_key), "click", None);
    assert!(first_attempt.is_err(), "approval should be required first");
    let err = first_attempt.unwrap_err();
    assert!(matches!(
        err.downcast_ref::<ActionError>(),
        Some(ActionError::HumanApprovalRequired { .. })
    ));

    let gateway = PageSessionGateway::new(Arc::clone(&page));
    let pending = gateway
        .pending_request()
        .expect("gateway should observe the pending request");

    gateway.approve(pending.id)?;
    assert!(page.pending_policy_approval().is_none());

    let retry = page.act(Some(target_id), Some(&target_key), "click", None);
    assert!(retry.is_ok(), "approved action should now succeed");

    let paid = page
        .evaluate_script("document.body.dataset.paid")?
        .value
        .and_then(|v| v.as_str().map(ToOwned::to_owned));
    assert_eq!(paid.as_deref(), Some("true"));

    Ok(())
}

#[cfg(test)]
fn find_button(node: &core_runtime::sre::state::SemanticNode) -> Option<(i64, String)> {
    if node.role == "button" {
        return Some((
            node.backend_node_id,
            node.stable_key.clone().unwrap_or_default(),
        ));
    }
    for child in &node.children {
        if let Some(found) = find_button(child) {
            return Some(found);
        }
    }
    None
}
