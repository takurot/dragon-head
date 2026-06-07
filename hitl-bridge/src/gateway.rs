//! Abstraction over the running browser session that the bridge resolves
//! human-approval requests against.
//!
//! [`ApprovalGateway`] lets the orchestration and HTTP layers be exercised in
//! tests without a live Chrome instance — [`MockGateway`] drives the same
//! trait surface from an in-memory queue.

use anyhow::Result;
use core_runtime::{ApprovalScope, OutcomeProjection, PageSession};
use std::sync::Arc;
use uuid::Uuid;

/// A pending human-approval request, identified by a bridge-minted [`Uuid`].
///
/// `core_runtime::PolicyApprovalRequest` has no stable identifier of its own —
/// the gateway mints `id` the first time it observes a given request (keyed by
/// `(rule_id, target_signature, action)`) so the chat notification, the lock
/// registry, and the audit trail can all refer to the same approval by ID.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingApproval {
    pub id: Uuid,
    pub rule_id: String,
    pub action: String,
    pub target_signature: String,
    pub scope: ApprovalScope,
    pub outcome: Option<OutcomeProjection>,
}

/// Resolves human-approval requests against a running session.
///
/// Implementors must be safe to share across the polling loop and the HTTP
/// handler threads.
pub trait ApprovalGateway: Send + Sync {
    /// Returns the current pending approval request, if any, assigning it a
    /// stable bridge-minted ID (reusing a previously-minted ID for the same
    /// underlying request).
    fn pending_request(&self) -> Option<PendingApproval>;

    /// Approve the pending request identified by `id`.
    ///
    /// Returns an error if `id` no longer matches the current pending request
    /// (e.g. it was already resolved or the underlying request changed).
    fn approve(&self, id: Uuid) -> Result<()>;

    /// Reject the pending request identified by `id`.
    ///
    /// Returns an error if `id` no longer matches the current pending request.
    fn reject(&self, id: Uuid) -> Result<()>;
}

/// Identifies a [`core_runtime::PolicyApprovalRequest`] independent of the
/// bridge-minted [`Uuid`], so repeated polls of the same request resolve to
/// the same ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RequestKey {
    rule_id: String,
    target_signature: String,
    action: String,
}

/// Single-slot cache mapping the most recently observed [`RequestKey`] to a
/// bridge-minted [`Uuid`].
///
/// Because the underlying session only ever tracks a single pending request at
/// a time, a single slot (rather than a full map) is sufficient and avoids
/// unbounded growth. The slot is cleared whenever no request is pending — see
/// [`clear`](Self::clear) — so that a *later* approval cycle on the same
/// control (same `rule_id`/`target_signature`/`action`) mints a fresh ID
/// rather than reusing one the lock registry has already recorded as resolved.
#[derive(Default)]
struct MintedIdCache {
    slot: Option<(RequestKey, Uuid)>,
}

impl MintedIdCache {
    fn id_for(&mut self, key: &RequestKey) -> Uuid {
        if let Some((existing_key, id)) = &self.slot {
            if existing_key == key {
                return *id;
            }
        }
        let id = Uuid::new_v4();
        self.slot = Some((key.clone(), id));
        id
    }

    /// Drops the cached ID. Call when no request is pending, so a future
    /// request with the same key — a fresh approval cycle on the same
    /// control — mints a new UUID instead of reusing one the lock registry
    /// already considers resolved (which would make the bridge treat a brand
    /// new request as a stale, already-decided interaction).
    fn clear(&mut self) {
        self.slot = None;
    }
}

/// [`ApprovalGateway`] backed by a live `core_runtime::PageSession`.
pub struct PageSessionGateway {
    session: Arc<PageSession>,
    minted: std::sync::Mutex<MintedIdCache>,
}

impl PageSessionGateway {
    pub fn new(session: Arc<PageSession>) -> Self {
        Self {
            session,
            minted: std::sync::Mutex::new(MintedIdCache::default()),
        }
    }

    fn id_for(&self, key: &RequestKey) -> Uuid {
        self.minted
            .lock()
            .expect("minted-id mutex poisoned")
            .id_for(key)
    }

    /// Returns the bridge-minted ID for the current pending request, if `id`
    /// still matches it. Used to reject stale interactions (e.g. a button
    /// press for a request that has since been superseded).
    fn current_id(&self) -> Option<Uuid> {
        let pending = self.session.pending_policy_approval()?;
        let key = RequestKey {
            rule_id: pending.rule_id,
            target_signature: pending.target_signature,
            action: pending.action,
        };
        Some(self.id_for(&key))
    }
}

impl ApprovalGateway for PageSessionGateway {
    fn pending_request(&self) -> Option<PendingApproval> {
        let Some(pending) = self.session.pending_policy_approval() else {
            self.minted
                .lock()
                .expect("minted-id mutex poisoned")
                .clear();
            return None;
        };
        let key = RequestKey {
            rule_id: pending.rule_id.clone(),
            target_signature: pending.target_signature.clone(),
            action: pending.action.clone(),
        };
        let id = self.id_for(&key);
        Some(PendingApproval {
            id,
            rule_id: pending.rule_id,
            action: pending.action,
            target_signature: pending.target_signature,
            scope: pending.scope,
            outcome: pending.outcome,
        })
    }

    fn approve(&self, id: Uuid) -> Result<()> {
        if self.current_id() != Some(id) {
            anyhow::bail!("Approval request {id} is no longer pending");
        }
        self.session.approve_pending_policy_action()
    }

    fn reject(&self, id: Uuid) -> Result<()> {
        if self.current_id() != Some(id) {
            anyhow::bail!("Approval request {id} is no longer pending");
        }
        self.session.reject_pending_policy_action()
    }
}

/// In-memory [`ApprovalGateway`] for tests — drives the same trait surface as
/// [`PageSessionGateway`] without a browser.
pub mod mock {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Resolution {
        Approved,
        Rejected,
    }

    #[derive(Default)]
    struct State {
        pending: Option<PendingApproval>,
        resolutions: Vec<(Uuid, Resolution)>,
    }

    /// Mock gateway seeded with a single pending request; records whatever
    /// resolution (approve/reject) the bridge applies to it.
    pub struct MockGateway {
        state: Mutex<State>,
    }

    impl MockGateway {
        pub fn new(pending: Option<PendingApproval>) -> Self {
            Self {
                state: Mutex::new(State {
                    pending,
                    resolutions: Vec::new(),
                }),
            }
        }

        /// Resolutions recorded so far, in application order.
        pub fn resolutions(&self) -> Vec<(Uuid, Resolution)> {
            self.state
                .lock()
                .expect("mock gateway mutex poisoned")
                .resolutions
                .clone()
        }

        fn resolve(&self, id: Uuid, resolution: Resolution) -> Result<()> {
            let mut state = self.state.lock().expect("mock gateway mutex poisoned");
            match &state.pending {
                Some(pending) if pending.id == id => {
                    state.pending = None;
                    state.resolutions.push((id, resolution));
                    Ok(())
                }
                _ => anyhow::bail!("Approval request {id} is no longer pending"),
            }
        }
    }

    impl ApprovalGateway for MockGateway {
        fn pending_request(&self) -> Option<PendingApproval> {
            self.state
                .lock()
                .expect("mock gateway mutex poisoned")
                .pending
                .clone()
        }

        fn approve(&self, id: Uuid) -> Result<()> {
            self.resolve(id, Resolution::Approved)
        }

        fn reject(&self, id: Uuid) -> Result<()> {
            self.resolve(id, Resolution::Rejected)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::{MockGateway, Resolution};
    use super::*;

    fn sample_pending(id: Uuid) -> PendingApproval {
        PendingApproval {
            id,
            rule_id: "approve-pay".to_string(),
            action: "click".to_string(),
            target_signature: "sig-123".to_string(),
            scope: ApprovalScope::ActionOnly,
            outcome: Some(OutcomeProjection {
                projected_amount: Some(900.0),
                risk_level: core_runtime::RiskLevel::High,
            }),
        }
    }

    fn sample_key() -> RequestKey {
        RequestKey {
            rule_id: "approve-pay".to_string(),
            target_signature: "sig-123".to_string(),
            action: "click".to_string(),
        }
    }

    #[test]
    fn minted_id_cache_returns_the_same_id_for_repeated_lookups_of_the_same_key() {
        let mut cache = MintedIdCache::default();
        let key = sample_key();

        let first = cache.id_for(&key);
        let second = cache.id_for(&key);

        assert_eq!(first, second);
    }

    #[test]
    fn minted_id_cache_mints_a_distinct_id_for_a_different_key() {
        let mut cache = MintedIdCache::default();
        let other_key = RequestKey {
            rule_id: "approve-pay".to_string(),
            target_signature: "sig-456".to_string(),
            action: "click".to_string(),
        };

        let first = cache.id_for(&sample_key());
        let second = cache.id_for(&other_key);

        assert_ne!(first, second);
    }

    #[test]
    fn minted_id_cache_mints_a_fresh_id_after_clear_for_a_new_approval_cycle_on_the_same_control() {
        let mut cache = MintedIdCache::default();
        let key = sample_key();

        let first_cycle = cache.id_for(&key);
        cache.clear();
        let second_cycle = cache.id_for(&key);

        assert_ne!(
            first_cycle, second_cycle,
            "a later approval cycle on the same (rule_id, target_signature, action) \
             must mint a new ID — reusing the old one would make the lock registry \
             treat the fresh request as already resolved"
        );
    }

    #[test]
    fn mock_gateway_reports_seeded_pending_request() {
        let id = Uuid::new_v4();
        let gateway = MockGateway::new(Some(sample_pending(id)));

        let pending = gateway.pending_request().expect("pending request");
        assert_eq!(pending.id, id);
        assert_eq!(pending.rule_id, "approve-pay");
    }

    #[test]
    fn mock_gateway_approve_clears_pending_and_records_resolution() {
        let id = Uuid::new_v4();
        let gateway = MockGateway::new(Some(sample_pending(id)));

        gateway.approve(id).expect("approve should succeed");

        assert!(gateway.pending_request().is_none());
        assert_eq!(gateway.resolutions(), vec![(id, Resolution::Approved)]);
    }

    #[test]
    fn mock_gateway_reject_clears_pending_and_records_resolution() {
        let id = Uuid::new_v4();
        let gateway = MockGateway::new(Some(sample_pending(id)));

        gateway.reject(id).expect("reject should succeed");

        assert!(gateway.pending_request().is_none());
        assert_eq!(gateway.resolutions(), vec![(id, Resolution::Rejected)]);
    }

    #[test]
    fn mock_gateway_rejects_stale_id() {
        let real_id = Uuid::new_v4();
        let stale_id = Uuid::new_v4();
        let gateway = MockGateway::new(Some(sample_pending(real_id)));

        let result = gateway.approve(stale_id);

        assert!(result.is_err());
        assert!(gateway.pending_request().is_some());
        assert!(gateway.resolutions().is_empty());
    }

    #[test]
    fn mock_gateway_with_no_pending_request_resolves_to_none() {
        let gateway = MockGateway::new(None);

        assert!(gateway.pending_request().is_none());
        assert!(gateway.approve(Uuid::new_v4()).is_err());
    }
}
