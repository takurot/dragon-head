//! Orchestration: polls the gateway for new approval requests, notifies chat,
//! and resolves interactions through the lock registry, gateway, audit trail,
//! and chat notifier — in that order, so a lock win is always followed by a
//! durable record before the chat message is updated.

use anyhow::Result;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::audit::{AuditDecision, AuditRecord, BridgeAuditTrail};
use crate::gateway::ApprovalGateway;
use crate::lock::{ClaimOutcome, Decision, SessionLockRegistry};
use crate::notifier::{ApprovalNotification, ChatNotifier};

fn epoch_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Wires the gateway, notifier, lock registry, and audit trail together.
///
/// Shared between the polling loop ([`Bridge::poll_once`]) and the HTTP
/// handler ([`Bridge::resolve`]) — both paths funnel through `resolve` so the
/// "claim, mutate, audit, respond" ordering is enforced exactly once.
pub struct Bridge {
    gateway: Arc<dyn ApprovalGateway>,
    notifier: Arc<dyn ChatNotifier>,
    locks: SessionLockRegistry,
    audit: BridgeAuditTrail,
    /// Maps a request ID to the notifier-issued token for its prompt message,
    /// so [`Bridge::resolve`] can update the original message in place.
    tokens: Mutex<std::collections::HashMap<Uuid, String>>,
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
            locks: SessionLockRegistry::new(),
            audit,
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
            let tokens = self.tokens.lock().expect("token map mutex poisoned");
            if tokens.contains_key(&pending.id) {
                return Ok(None);
            }
        }

        let notification = ApprovalNotification {
            id: pending.id,
            rule_id: pending.rule_id.clone(),
            action: pending.action.clone(),
            outcome: pending.outcome.clone(),
            som_image_png: None,
        };
        let token = self.notifier.notify(&notification)?;

        self.tokens
            .lock()
            .expect("token map mutex poisoned")
            .insert(pending.id, token);

        Ok(Some(pending.id))
    }

    /// Resolves the request `id` as `decision`, attributed to `decided_by`.
    ///
    /// Enforces the session-level exclusive lock first: if another reviewer
    /// already claimed this request, no gateway/audit/notifier mutation
    /// happens and an error describing the existing resolution is returned —
    /// callers should surface this as "already resolved by @X" rather than a
    /// generic failure.
    pub fn resolve(&self, id: Uuid, decision: Decision, decided_by: &str) -> Result<()> {
        match self.locks.try_claim(id, decided_by, decision) {
            ClaimOutcome::Won => {}
            ClaimOutcome::LostTo {
                decided_by,
                decision,
            } => {
                anyhow::bail!("request {id} was already resolved by {decided_by} ({decision:?})");
            }
        }

        let outcome_projection = self.gateway.pending_request().and_then(|pending| {
            if pending.id == id {
                pending.outcome
            } else {
                None
            }
        });

        match decision {
            Decision::Approved => self.gateway.approve(id)?,
            Decision::Rejected => self.gateway.reject(id)?,
        }

        self.audit.record(&AuditRecord {
            id,
            decision: AuditDecision::from(decision),
            decided_by: decided_by.to_string(),
            decided_at_ms: epoch_millis(),
            outcome_projection,
        })?;

        let token = self
            .tokens
            .lock()
            .expect("token map mutex poisoned")
            .get(&id)
            .cloned();
        if let Some(token) = token {
            // Only drop the token once the chat update succeeds — if
            // `respond` fails (e.g. a transient Slack API error), the token
            // is the only handle that lets a retry repair the now-stale
            // prompt for a request whose gateway mutation and audit record
            // have already been durably committed.
            self.notifier.respond(&token, decision, decided_by)?;
            self.tokens
                .lock()
                .expect("token map mutex poisoned")
                .remove(&id);
        }

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
    use tempfile::tempdir;

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

    fn bridge_audit_records(dir: &tempfile::TempDir) -> Vec<AuditRecord> {
        BridgeAuditTrail::new(dir.path().join("audit.ndjson"))
            .read_all()
            .expect("read audit trail")
    }
}
