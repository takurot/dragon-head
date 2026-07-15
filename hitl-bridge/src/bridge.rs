//! Orchestration: polls the gateway for new approval requests, notifies chat,
//! and resolves interactions through resumable gateway, audit, and notifier
//! phases. The first decision owns the request; an exact retry by the same
//! reviewer resumes at the first incomplete phase without duplicating earlier
//! side effects.

use anyhow::Result;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::audit::{AuditDecision, AuditRecord, BridgeAuditTrail};
use crate::gateway::ApprovalGateway;
use crate::lock::Decision;
use crate::notifier::{ApprovalNotification, ChatNotifier};

const MAX_RESOLUTION_ENTRIES: usize = 1024;

enum NotificationState {
    Notifying,
    Notified(String),
}

struct ResolutionProgress {
    decision: Decision,
    record: AuditRecord,
    gateway_applied: bool,
    audited: bool,
    chat_updated: bool,
}

impl ResolutionProgress {
    fn new(
        id: Uuid,
        decision: Decision,
        decided_by: &str,
        outcome_projection: Option<core_runtime::OutcomeProjection>,
    ) -> Self {
        Self {
            decision,
            record: AuditRecord {
                id,
                decision: AuditDecision::from(decision),
                decided_by: decided_by.to_string(),
                decided_at_ms: epoch_millis(),
                outcome_projection,
            },
            gateway_applied: false,
            audited: false,
            chat_updated: false,
        }
    }

    fn is_same_decision(&self, decision: Decision, decided_by: &str) -> bool {
        self.decision == decision && self.record.decided_by == decided_by
    }
}

#[derive(Default)]
struct ResolutionRegistry {
    entries: HashMap<Uuid, Arc<Mutex<ResolutionProgress>>>,
    terminal_order: VecDeque<Uuid>,
}

impl ResolutionRegistry {
    fn make_room_for_claim(&mut self) -> bool {
        while self.entries.len() >= MAX_RESOLUTION_ENTRIES {
            let Some(expired) = self.terminal_order.pop_front() else {
                return false;
            };
            self.entries.remove(&expired);
        }
        true
    }

    fn mark_terminal(&mut self, id: Uuid) {
        self.terminal_order.push_back(id);
        while self.terminal_order.len() > MAX_RESOLUTION_ENTRIES {
            if let Some(expired) = self.terminal_order.pop_front() {
                self.entries.remove(&expired);
            }
        }
    }
}

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Wires the gateway, notifier, resolution registry, and audit trail together.
///
/// Shared between the polling loop ([`Bridge::poll_once`]) and the HTTP
/// handler ([`Bridge::resolve`]) — both paths funnel through `resolve` so the
/// "claim, mutate, audit, respond" phases are serialized and resumed safely.
pub struct Bridge {
    gateway: Arc<dyn ApprovalGateway>,
    notifier: Arc<dyn ChatNotifier>,
    audit: BridgeAuditTrail,
    resolutions: Mutex<ResolutionRegistry>,
    /// Maps a request ID to the notifier-issued token for its prompt message,
    /// so [`Bridge::resolve`] can update the original message in place.
    tokens: Mutex<HashMap<Uuid, NotificationState>>,
}

impl Bridge {
    pub fn new(
        gateway: Arc<dyn ApprovalGateway>,
        notifier: Arc<dyn ChatNotifier>,
        audit: BridgeAuditTrail,
    ) -> Self {
        Self {
            gateway,
            notifier,
            audit,
            resolutions: Mutex::new(ResolutionRegistry::default()),
            tokens: Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Checks for a pending request and, if it has not been notified yet,
    /// posts the prompt and remembers the resulting token.
    ///
    /// Returns the request ID that was newly notified, if any.
    pub fn poll_once(&self) -> Result<Option<Uuid>> {
        let Some(pending) = self.gateway.pending_request() else {
            return Ok(None);
        };

        {
            let mut tokens = self.tokens.lock().expect("token map mutex poisoned");
            if tokens.contains_key(&pending.id) {
                return Ok(None);
            }
            tokens.insert(pending.id, NotificationState::Notifying);
        }

        let notification = ApprovalNotification {
            id: pending.id,
            rule_id: pending.rule_id.clone(),
            action: pending.action.clone(),
            outcome: pending.outcome.clone(),
            som_image_png: None,
        };
        let token = match self.notifier.notify(&notification) {
            Ok(token) => token,
            Err(err) => {
                self.tokens
                    .lock()
                    .expect("token map mutex poisoned")
                    .remove(&pending.id);
                return Err(err);
            }
        };

        self.tokens
            .lock()
            .expect("token map mutex poisoned")
            .insert(pending.id, NotificationState::Notified(token));

        Ok(Some(pending.id))
    }

    /// Resolves the request `id` as `decision`, attributed to `decided_by`.
    ///
    /// The first reviewer and decision atomically own the request. If a phase
    /// fails, that exact reviewer/decision pair may retry; competing decisions
    /// never mutate the gateway, audit trail, or notifier.
    pub fn resolve(&self, id: Uuid, decision: Decision, decided_by: &str) -> Result<()> {
        let progress = {
            let mut resolutions = self
                .resolutions
                .lock()
                .expect("resolution registry mutex poisoned");
            if let Some(progress) = resolutions.entries.get(&id) {
                Arc::clone(progress)
            } else {
                if !resolutions.make_room_for_claim() {
                    anyhow::bail!(
                        "resolution registry capacity ({MAX_RESOLUTION_ENTRIES}) reached"
                    );
                }
                let outcome_projection = self.gateway.pending_request().and_then(|pending| {
                    if pending.id == id {
                        pending.outcome
                    } else {
                        None
                    }
                });
                let progress = Arc::new(Mutex::new(ResolutionProgress::new(
                    id,
                    decision,
                    decided_by,
                    outcome_projection,
                )));
                resolutions.entries.insert(id, Arc::clone(&progress));
                progress
            }
        };

        let mut progress_guard = progress.lock().expect("resolution progress mutex poisoned");
        if !progress_guard.is_same_decision(decision, decided_by) || progress_guard.chat_updated {
            anyhow::bail!(
                "request {id} was already resolved by {} ({:?})",
                progress_guard.record.decided_by,
                progress_guard.decision
            );
        }

        if !progress_guard.gateway_applied {
            match decision {
                Decision::Approved => self.gateway.approve(id)?,
                Decision::Rejected => self.gateway.reject(id)?,
            }
            progress_guard.gateway_applied = true;
        }

        if !progress_guard.audited {
            self.audit.record(&progress_guard.record)?;
            progress_guard.audited = true;
        }

        let notification = {
            let tokens = self.tokens.lock().expect("token map mutex poisoned");
            match tokens.get(&id) {
                Some(NotificationState::Notifying) => {
                    anyhow::bail!("request {id} notification is still being posted")
                }
                Some(NotificationState::Notified(token)) => Some(token.clone()),
                None => None,
            }
        };
        if let Some(token) = notification {
            self.notifier.respond(&token, decision, decided_by)?;
            self.tokens
                .lock()
                .expect("token map mutex poisoned")
                .remove(&id);
        }
        progress_guard.chat_updated = true;
        drop(progress_guard);

        self.resolutions
            .lock()
            .expect("resolution registry mutex poisoned")
            .mark_terminal(id);
        Ok(())
    }
}

/// Runs [`Bridge::poll_once`] on a fixed interval until `should_stop` returns
/// `true`. Polling errors are logged and do not stop the loop — a transient
/// failure to reach the gateway should not take the bridge down.
pub fn run_poll_loop(bridge: &Bridge, interval: Duration, mut should_stop: impl FnMut() -> bool) {
    while !should_stop() {
        if let Err(err) = bridge.poll_once() {
            tracing::warn!(error = %err, "poll cycle failed");
        }
        std::thread::sleep(interval);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::mock::MockGateway;
    use crate::gateway::PendingApproval;
    use crate::notifier::mock::{Call, MockNotifier};
    use core_runtime::{ApprovalScope, OutcomeProjection, RiskLevel};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    struct FailOnceNotifier {
        attempts: AtomicUsize,
        calls: Mutex<Vec<Call>>,
    }

    impl FailOnceNotifier {
        fn new() -> Self {
            Self {
                attempts: AtomicUsize::new(0),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn attempts(&self) -> usize {
            self.attempts.load(Ordering::SeqCst)
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.lock().expect("notifier calls mutex").clone()
        }
    }

    impl ChatNotifier for FailOnceNotifier {
        fn notify(&self, notification: &ApprovalNotification) -> Result<String> {
            self.calls
                .lock()
                .expect("notifier calls mutex")
                .push(Call::Notify(notification.clone()));
            Ok(format!("mock-channel:{}", notification.id))
        }

        fn respond(&self, token: &str, decision: Decision, decided_by: &str) -> Result<()> {
            if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                anyhow::bail!("injected notifier failure");
            }
            self.calls
                .lock()
                .expect("notifier calls mutex")
                .push(Call::Respond {
                    token: token.to_string(),
                    decision,
                    decided_by: decided_by.to_string(),
                });
            Ok(())
        }
    }

    struct FailOnceGateway {
        pending: Mutex<Option<PendingApproval>>,
        attempts: AtomicUsize,
        resolutions: Mutex<Vec<Decision>>,
    }

    struct AlwaysFailGateway;

    impl ApprovalGateway for AlwaysFailGateway {
        fn pending_request(&self) -> Option<PendingApproval> {
            None
        }

        fn approve(&self, _id: Uuid) -> Result<()> {
            anyhow::bail!("injected permanent gateway failure")
        }

        fn reject(&self, _id: Uuid) -> Result<()> {
            anyhow::bail!("injected permanent gateway failure")
        }
    }

    impl FailOnceGateway {
        fn new(pending: PendingApproval) -> Self {
            Self {
                pending: Mutex::new(Some(pending)),
                attempts: AtomicUsize::new(0),
                resolutions: Mutex::new(Vec::new()),
            }
        }
    }

    impl ApprovalGateway for FailOnceGateway {
        fn pending_request(&self) -> Option<PendingApproval> {
            self.pending.lock().expect("pending mutex").clone()
        }

        fn approve(&self, id: Uuid) -> Result<()> {
            if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                anyhow::bail!("injected gateway failure");
            }
            let pending = self.pending.lock().expect("pending mutex").take();
            if pending.as_ref().map(|pending| pending.id) != Some(id) {
                anyhow::bail!("request is no longer pending");
            }
            self.resolutions
                .lock()
                .expect("resolutions mutex")
                .push(Decision::Approved);
            Ok(())
        }

        fn reject(&self, _id: Uuid) -> Result<()> {
            anyhow::bail!("unexpected rejection")
        }
    }

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

    fn bridge_with(
        id: Uuid,
        dir: &tempfile::TempDir,
    ) -> (Bridge, Arc<MockGateway>, Arc<MockNotifier>) {
        let gateway = Arc::new(MockGateway::new(Some(sample_pending(id))));
        let notifier = Arc::new(MockNotifier::new());
        let audit = BridgeAuditTrail::new(dir.path().join("audit.ndjson"));
        let bridge = Bridge::new(
            gateway.clone() as Arc<dyn ApprovalGateway>,
            notifier.clone() as Arc<dyn ChatNotifier>,
            audit,
        );
        (bridge, gateway, notifier)
    }

    #[test]
    fn poll_once_notifies_new_request_and_remembers_token() {
        let dir = tempdir().expect("tempdir");
        let id = Uuid::new_v4();
        let (bridge, _gateway, notifier) = bridge_with(id, &dir);

        let notified = bridge.poll_once().expect("poll should succeed");

        assert_eq!(notified, Some(id));
        assert_eq!(notifier.calls().len(), 1);
        assert!(matches!(notifier.calls()[0], Call::Notify(_)));
    }

    #[test]
    fn poll_once_does_not_renotify_the_same_request() {
        let dir = tempdir().expect("tempdir");
        let id = Uuid::new_v4();
        let (bridge, _gateway, notifier) = bridge_with(id, &dir);

        bridge.poll_once().expect("first poll");
        let second = bridge.poll_once().expect("second poll");

        assert_eq!(second, None);
        assert_eq!(
            notifier.calls().len(),
            1,
            "notify must be called exactly once"
        );
    }

    #[test]
    fn concurrent_polls_post_only_one_prompt() {
        let dir = tempdir().expect("tempdir");
        let id = Uuid::new_v4();
        let (bridge, _gateway, notifier) = bridge_with(id, &dir);
        let bridge = Arc::new(bridge);

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let bridge = Arc::clone(&bridge);
                std::thread::spawn(move || bridge.poll_once())
            })
            .collect();
        let newly_notified = handles
            .into_iter()
            .map(|handle| handle.join().expect("poll thread").expect("poll result"))
            .filter(Option::is_some)
            .count();

        assert_eq!(newly_notified, 1);
        assert_eq!(
            notifier
                .calls()
                .iter()
                .filter(|call| matches!(call, Call::Notify(_)))
                .count(),
            1
        );
    }

    #[test]
    fn resolve_approves_audits_and_responds_exactly_once() {
        let dir = tempdir().expect("tempdir");
        let id = Uuid::new_v4();
        let (bridge, gateway, notifier) = bridge_with(id, &dir);

        bridge.poll_once().expect("poll");
        bridge
            .resolve(id, Decision::Approved, "alice")
            .expect("resolve should succeed");

        assert_eq!(gateway.resolutions().len(), 1);
        let records = bridge_audit_records(&dir);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].decided_by, "alice");
        assert_eq!(records[0].decision, AuditDecision::Approved);
        assert!(records[0].outcome_projection.is_some());

        let respond_count = notifier
            .calls()
            .iter()
            .filter(|call| matches!(call, Call::Respond { .. }))
            .count();
        assert_eq!(respond_count, 1);
    }

    #[test]
    fn second_resolve_attempt_loses_lock_and_does_not_double_mutate() {
        let dir = tempdir().expect("tempdir");
        let id = Uuid::new_v4();
        let (bridge, gateway, _notifier) = bridge_with(id, &dir);

        bridge.poll_once().expect("poll");
        bridge
            .resolve(id, Decision::Approved, "alice")
            .expect("first resolve wins");
        let second = bridge.resolve(id, Decision::Rejected, "bob");

        assert!(
            second.is_err(),
            "second resolution must be rejected by the lock"
        );
        assert_eq!(
            gateway.resolutions().len(),
            1,
            "gateway must be mutated exactly once"
        );
        assert_eq!(
            bridge_audit_records(&dir).len(),
            1,
            "audit trail must have exactly one record"
        );
    }

    #[test]
    fn resolve_keeps_the_chat_token_when_the_chat_update_fails() {
        let dir = tempdir().expect("tempdir");
        let id = Uuid::new_v4();
        let gateway = Arc::new(MockGateway::new(Some(sample_pending(id))));
        let notifier = Arc::new(MockNotifier::new_failing_respond());
        let audit = BridgeAuditTrail::new(dir.path().join("audit.ndjson"));
        let bridge = Bridge::new(
            gateway.clone() as Arc<dyn ApprovalGateway>,
            notifier.clone() as Arc<dyn ChatNotifier>,
            audit,
        );

        bridge.poll_once().expect("poll");
        let result = bridge.resolve(id, Decision::Approved, "alice");

        assert!(
            result.is_err(),
            "resolve must surface the chat-update failure"
        );
        assert_eq!(
            gateway.resolutions().len(),
            1,
            "the decision must still be applied to the gateway"
        );
        assert_eq!(
            bridge_audit_records(&dir).len(),
            1,
            "the decision must still be durably audited"
        );
        assert_eq!(
            bridge
                .tokens
                .lock()
                .expect("token map mutex poisoned")
                .len(),
            1,
            "the chat token must be retained so a retry can repair the stale prompt"
        );
    }

    #[test]
    fn resolve_retries_only_the_failed_notifier_phase() {
        let dir = tempdir().expect("tempdir");
        let id = Uuid::new_v4();
        let gateway = Arc::new(MockGateway::new(Some(sample_pending(id))));
        let notifier = Arc::new(FailOnceNotifier::new());
        let audit = BridgeAuditTrail::new(dir.path().join("audit.ndjson"));
        let bridge = Bridge::new(
            gateway.clone() as Arc<dyn ApprovalGateway>,
            notifier.clone() as Arc<dyn ChatNotifier>,
            audit,
        );

        bridge.poll_once().expect("poll");
        assert!(bridge.resolve(id, Decision::Approved, "alice").is_err());
        let competing = bridge.resolve(id, Decision::Rejected, "bob");
        assert!(
            competing.is_err(),
            "a competing decision must not take over a failed notifier phase"
        );
        let changed_decision = bridge.resolve(id, Decision::Rejected, "alice");
        assert!(
            changed_decision.is_err(),
            "the owning reviewer must not change the claimed decision during retry"
        );
        bridge
            .resolve(id, Decision::Approved, "alice")
            .expect("the original decision may repair the chat update");

        assert_eq!(gateway.resolutions().len(), 1);
        assert_eq!(bridge_audit_records(&dir).len(), 1);
        assert_eq!(notifier.attempts(), 2);
        assert_eq!(
            notifier
                .calls()
                .iter()
                .filter(|call| matches!(call, Call::Respond { .. }))
                .count(),
            1
        );
        assert!(bridge.tokens.lock().expect("token map mutex").is_empty());
    }

    #[test]
    fn resolve_retries_the_gateway_phase_for_the_original_decision() {
        let dir = tempdir().expect("tempdir");
        let id = Uuid::new_v4();
        let gateway = Arc::new(FailOnceGateway::new(sample_pending(id)));
        let notifier = Arc::new(MockNotifier::new());
        let audit = BridgeAuditTrail::new(dir.path().join("audit.ndjson"));
        let bridge = Bridge::new(
            gateway.clone() as Arc<dyn ApprovalGateway>,
            notifier.clone() as Arc<dyn ChatNotifier>,
            audit,
        );

        bridge.poll_once().expect("poll");
        assert!(bridge.resolve(id, Decision::Approved, "alice").is_err());
        bridge
            .resolve(id, Decision::Approved, "alice")
            .expect("the original decision may retry the gateway");

        assert_eq!(gateway.attempts.load(Ordering::SeqCst), 2);
        assert_eq!(
            gateway.resolutions.lock().expect("resolutions mutex").len(),
            1
        );
        assert_eq!(bridge_audit_records(&dir).len(), 1);
    }

    #[test]
    fn resolve_retries_audit_without_reapplying_the_gateway() {
        let dir = tempdir().expect("tempdir");
        let audit_dir = dir.path().join("missing");
        let id = Uuid::new_v4();
        let gateway = Arc::new(MockGateway::new(Some(sample_pending(id))));
        let notifier = Arc::new(MockNotifier::new());
        let bridge = Bridge::new(
            gateway.clone() as Arc<dyn ApprovalGateway>,
            notifier as Arc<dyn ChatNotifier>,
            BridgeAuditTrail::new(audit_dir.join("audit.ndjson")),
        );

        bridge.poll_once().expect("poll");
        assert!(bridge.resolve(id, Decision::Approved, "alice").is_err());
        std::fs::create_dir(&audit_dir).expect("create audit directory");
        bridge
            .resolve(id, Decision::Approved, "alice")
            .expect("the original decision may retry the audit phase");

        assert_eq!(gateway.resolutions().len(), 1);
        let records = BridgeAuditTrail::new(audit_dir.join("audit.ndjson"))
            .read_all()
            .expect("read audit records");
        assert_eq!(records.len(), 1);
        assert!(records[0].outcome_projection.is_some());
    }

    #[test]
    fn terminal_resolution_tracking_is_bounded() {
        let mut registry = ResolutionRegistry::default();
        for index in 0..=MAX_RESOLUTION_ENTRIES {
            let id = Uuid::new_v4();
            let mut progress =
                ResolutionProgress::new(id, Decision::Approved, &format!("reviewer-{index}"), None);
            progress.chat_updated = true;
            registry.entries.insert(id, Arc::new(Mutex::new(progress)));
            registry.mark_terminal(id);
        }

        assert_eq!(registry.entries.len(), MAX_RESOLUTION_ENTRIES);
        assert_eq!(registry.terminal_order.len(), MAX_RESOLUTION_ENTRIES);
    }

    #[test]
    fn completed_resolution_capacity_makes_room_for_a_new_claim() {
        let dir = tempdir().expect("tempdir");
        let id = Uuid::new_v4();
        let gateway = Arc::new(MockGateway::new(Some(sample_pending(id))));
        let bridge = Bridge::new(
            gateway.clone() as Arc<dyn ApprovalGateway>,
            Arc::new(MockNotifier::new()) as Arc<dyn ChatNotifier>,
            BridgeAuditTrail::new(dir.path().join("audit.ndjson")),
        );
        {
            let mut registry = bridge
                .resolutions
                .lock()
                .expect("resolution registry mutex");
            for index in 0..MAX_RESOLUTION_ENTRIES {
                let terminal_id = Uuid::new_v4();
                let mut progress = ResolutionProgress::new(
                    terminal_id,
                    Decision::Approved,
                    &format!("reviewer-{index}"),
                    None,
                );
                progress.chat_updated = true;
                registry
                    .entries
                    .insert(terminal_id, Arc::new(Mutex::new(progress)));
                registry.mark_terminal(terminal_id);
            }
        }

        bridge
            .resolve(id, Decision::Approved, "alice")
            .expect("a completed entry should be evicted for the new claim");

        let resolutions = gateway.resolutions();
        assert_eq!(resolutions.len(), 1);
        assert_eq!(resolutions[0].0, id);
        let registry = bridge
            .resolutions
            .lock()
            .expect("resolution registry mutex");
        assert_eq!(registry.entries.len(), MAX_RESOLUTION_ENTRIES);
        assert!(registry.entries.contains_key(&id));
    }

    #[test]
    fn incomplete_resolution_tracking_rejects_new_claims_at_capacity() {
        let dir = tempdir().expect("tempdir");
        let bridge = Bridge::new(
            Arc::new(AlwaysFailGateway) as Arc<dyn ApprovalGateway>,
            Arc::new(MockNotifier::new()) as Arc<dyn ChatNotifier>,
            BridgeAuditTrail::new(dir.path().join("audit.ndjson")),
        );

        for _ in 0..MAX_RESOLUTION_ENTRIES {
            assert!(bridge
                .resolve(Uuid::new_v4(), Decision::Approved, "alice")
                .is_err());
        }
        let overflow = bridge.resolve(Uuid::new_v4(), Decision::Approved, "alice");

        assert!(overflow
            .expect_err("new claims must be rejected at capacity")
            .to_string()
            .contains("capacity"));
        assert_eq!(
            bridge
                .resolutions
                .lock()
                .expect("resolution registry mutex")
                .entries
                .len(),
            MAX_RESOLUTION_ENTRIES
        );
    }

    fn bridge_audit_records(dir: &tempfile::TempDir) -> Vec<AuditRecord> {
        BridgeAuditTrail::new(dir.path().join("audit.ndjson"))
            .read_all()
            .expect("read audit trail")
    }
}
