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
/// The gateway maps the runtime's approval-cycle identity and session to a bridge ID,
/// so the chat notification, lock registry, and audit trail refer to exactly the
/// request that was reviewed, including replacements with identical display data.
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

/// Identity of one `PageSession` allocation. Holds a `Weak` so the address cannot be recycled
/// for a different session while the token is alive.
#[derive(Debug, Clone)]
struct SessionToken(std::sync::Weak<dyn std::any::Any + Send + Sync>);

impl SessionToken {
    fn of<T: std::any::Any + Send + Sync>(session: &Arc<T>) -> Self {
        let erased: Arc<dyn std::any::Any + Send + Sync> = Arc::clone(session) as _;
        Self(Arc::downgrade(&erased))
    }
}

impl PartialEq for SessionToken {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_ptr() as *const () == other.0.as_ptr() as *const ()
    }
}

impl Eq for SessionToken {}

/// Identifies a [`core_runtime::browser::PolicyApprovalRequestSnapshot`] independent of the
/// bridge-minted [`Uuid`], so repeated polls of the same request resolve to
/// the same ID.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestKey {
    /// The session the request was observed on. A restart-driven session swap changes this, so
    /// an ID minted for the crashed session never validates against, or is reused for, a
    /// request on its replacement even if the rule/target/action happen to match.
    session: SessionToken,
    request_id: Uuid,
}

/// Single-slot cache mapping the most recently observed [`RequestKey`] to a
/// bridge-minted [`Uuid`].
///
/// Because the underlying session only ever tracks a single pending request at
/// a time, a single slot (rather than a full map) is sufficient and avoids
/// unbounded growth. Every replacement has a new runtime identity, so an unobserved
/// clear-and-replace cycle also mints a fresh bridge ID. Observing no pending request
/// clears the slot to discard the previous session token promptly.
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

    /// Read-only lookup for validation paths (e.g. checking a caller-supplied
    /// ID against the current pending request). Never mints a new ID as a
    /// side effect — minting here would let a validation check silently
    /// invalidate an ID a client already holds from [`Self::id_for`], turning
    /// a legitimate approve/reject into a spurious "no longer pending" error.
    fn peek(&self, key: &RequestKey) -> Option<Uuid> {
        match &self.slot {
            Some((existing_key, id)) if existing_key == key => Some(*id),
            _ => None,
        }
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

/// Returns the `PageSession` the gateway should act on right now. Called on every gateway
/// operation so a host that swaps its session (browser-restart recovery) is followed instead of
/// the gateway pinning the session that was live at construction time.
pub type SessionProvider = Arc<dyn Fn() -> Arc<PageSession> + Send + Sync>;

/// [`ApprovalGateway`] backed by a live `core_runtime::PageSession`.
pub struct PageSessionGateway {
    session: SessionProvider,
    minted: std::sync::Mutex<MintedIdCache>,
    #[cfg(test)]
    before_resolve: std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl PageSessionGateway {
    /// Gateway bound to one fixed session for its whole lifetime.
    pub fn new(session: Arc<PageSession>) -> Self {
        Self::with_provider(Arc::new(move || Arc::clone(&session)))
    }

    /// Gateway that re-resolves its session through `session` on every operation.
    pub fn with_provider(session: SessionProvider) -> Self {
        Self {
            session,
            minted: std::sync::Mutex::new(MintedIdCache::default()),
            #[cfg(test)]
            before_resolve: std::sync::Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn run_before_resolve(&self) {
        let callback = self.before_resolve.lock().unwrap().take();
        if let Some(callback) = callback {
            callback();
        }
    }

    fn id_for(&self, key: &RequestKey) -> Uuid {
        self.minted
            .lock()
            .expect("minted-id mutex poisoned")
            .id_for(key)
    }

    /// Validate the bridge ID without minting, then return the runtime identity that
    /// must still match when the decision commits under the session's approval lock.
    fn expected_request_id(&self, session: &Arc<PageSession>, id: Uuid) -> Option<Uuid> {
        let pending = session.pending_policy_approval_snapshot()?;
        let key = RequestKey {
            session: SessionToken::of(session),
            request_id: pending.id,
        };
        let current_id = self
            .minted
            .lock()
            .expect("minted-id mutex poisoned")
            .peek(&key);
        (current_id == Some(id)).then_some(pending.id)
    }
}

impl ApprovalGateway for PageSessionGateway {
    fn pending_request(&self) -> Option<PendingApproval> {
        let session = (self.session)();
        let Some(snapshot) = session.pending_policy_approval_snapshot() else {
            self.minted
                .lock()
                .expect("minted-id mutex poisoned")
                .clear();
            return None;
        };
        let key = RequestKey {
            session: SessionToken::of(&session),
            request_id: snapshot.id,
        };
        let id = self.id_for(&key);
        let pending = snapshot.request;
        Some(PendingApproval {
            id,
            rule_id: pending.rule_id,
            action: pending.action,
            target_signature: pending.target_signature,
            scope: pending.scope,
            outcome: pending.outcome,
        })
    }

    // Resolve the session once per operation: validating the ID on one session and applying the
    // decision to another (after a restart swap) would resolve an unvalidated request.
    fn approve(&self, id: Uuid) -> Result<()> {
        let session = (self.session)();
        let expected_id = self
            .expected_request_id(&session, id)
            .ok_or_else(|| anyhow::anyhow!("Approval request {id} is no longer pending"))?;
        #[cfg(test)]
        self.run_before_resolve();
        session
            .approve_pending_policy_request_if_current(expected_id, "hitl_bridge")
            .map(|_| ())
    }

    fn reject(&self, id: Uuid) -> Result<()> {
        let session = (self.session)();
        let expected_id = self
            .expected_request_id(&session, id)
            .ok_or_else(|| anyhow::anyhow!("Approval request {id} is no longer pending"))?;
        #[cfg(test)]
        self.run_before_resolve();
        session.reject_pending_policy_request_if_current(expected_id)
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

    fn approval_rule(scope: ApprovalScope, projected: bool) -> core_runtime::PolicyRule {
        core_runtime::PolicyRule {
            id: "approve-pay".to_string(),
            domain: None,
            path_prefix: None,
            role: None,
            text_regex: None,
            context_regex: None,
            action: core_runtime::PolicyAction::RequireHumanApproval,
            scope: Some(scope),
            outcome_projector: projected.then_some(core_runtime::OutcomeProjectorConfig {
                amount_regex: Some(r"\$(?P<amount>\d+)".to_string()),
                block_if_amount_exceeds: None,
                warn_if_amount_exceeds: Some(100.0),
            }),
        }
    }

    fn approval_page() -> Result<(core_runtime::BrowserClient, Arc<PageSession>)> {
        let client = core_runtime::BrowserClient::new()?;
        let page = Arc::new(client.new_page()?);
        page.navigate(&format!("data:text/html,{}", urlencoding::encode(
            r#"<html><body><div>Total: $900<button id="x">Pay X</button><button id="y">Pay Y</button></div></body></html>"#
        )))?;
        page.set_policy_rules(vec![approval_rule(ApprovalScope::ActionOnly, false)])?;
        Ok((client, page))
    }

    fn require_approval(page: &PageSession, id: &str) -> Result<()> {
        fn find<'a>(
            node: &'a core_runtime::sre::SemanticNode,
            id: &str,
        ) -> Option<&'a core_runtime::sre::SemanticNode> {
            if node
                .attributes
                .as_ref()
                .and_then(|attributes| attributes.get("id"))
                .map(String::as_str)
                == Some(id)
            {
                return Some(node);
            }
            node.children.iter().find_map(|child| find(child, id))
        }
        let state = page.capture_semantic_state(core_runtime::sre::LoadProfile::Interactive)?;
        let target = find(state.root(), id).expect("fixture target");
        let error = page
            .act(
                Some(target.backend_node_id),
                target.stable_key.as_deref(),
                "click",
                None,
            )
            .expect_err("fixture action must require approval");
        assert!(matches!(
            error.downcast_ref::<core_runtime::ActionError>(),
            Some(core_runtime::ActionError::HumanApprovalRequired { .. })
        ));
        Ok(())
    }

    fn replaced_request_is_not_resolved(
        approve: bool,
        replacement_target: &'static str,
    ) -> Result<()> {
        if test_bench_support::should_skip_browser_tests() {
            return Ok(());
        }
        let (_client, page) = approval_page()?;
        require_approval(&page, "x")?;
        let gateway = PageSessionGateway::new(Arc::clone(&page));
        let original = gateway.pending_request().expect("original request");
        let callback_page = Arc::clone(&page);
        *gateway.before_resolve.lock().unwrap() = Some(Box::new(move || {
            require_approval(&callback_page, replacement_target)
                .expect("replace pending after gateway validation");
        }));
        page.clear_audit_events();
        let result = if approve {
            gateway.approve(original.id)
        } else {
            gateway.reject(original.id)
        };
        assert!(
            result.is_err(),
            "a decision on X must not resolve replacement Y"
        );
        let replacement = page
            .pending_policy_approval()
            .expect("Y must remain pending");
        if replacement_target == "y" {
            assert_ne!(replacement.target_signature, original.target_signature);
        } else {
            assert_eq!(replacement.target_signature, original.target_signature);
        }
        assert!(!page.audit_events().iter().any(|event| matches!(event,
            core_runtime::audit::AuditEvent::HitlEvent { event_type, .. } if event_type == "approved" || event_type == "rejected"
        )), "stale decisions must not emit successful resolution audit");
        require_approval(&page, replacement_target)?;
        let current = gateway
            .pending_request()
            .expect("replacement still requires human decision");
        assert_ne!(current.id, original.id);
        gateway.approve(current.id)?;
        assert!(page.pending_policy_approval().is_none());
        Ok(())
    }

    #[test]
    fn gateway_approve_rejects_replacement_after_id_validation() -> Result<()> {
        replaced_request_is_not_resolved(true, "y")
    }

    #[test]
    fn gateway_reject_rejects_replacement_after_id_validation() -> Result<()> {
        replaced_request_is_not_resolved(false, "y")
    }

    #[test]
    fn gateway_approve_rejects_same_shape_replacement_after_id_validation() -> Result<()> {
        replaced_request_is_not_resolved(true, "x")
    }

    #[test]
    fn gateway_reject_rejects_same_shape_replacement_after_id_validation() -> Result<()> {
        replaced_request_is_not_resolved(false, "x")
    }

    #[test]
    fn core_expected_decisions_preserve_replacement_without_grant_or_audit() -> Result<()> {
        if test_bench_support::should_skip_browser_tests() {
            return Ok(());
        }
        let (_client, page) = approval_page()?;
        require_approval(&page, "x")?;
        let original = page.pending_policy_approval_snapshot().unwrap();
        assert_eq!(
            page.pending_policy_approval_snapshot().unwrap().id,
            original.id
        );
        require_approval(&page, "y")?;
        let replacement = page.pending_policy_approval_snapshot().unwrap();
        page.clear_audit_events();
        assert!(page
            .approve_pending_policy_request_if_current(original.id, "hitl_bridge")
            .is_err());
        assert!(page
            .reject_pending_policy_request_if_current(original.id)
            .is_err());
        assert_eq!(page.pending_policy_approval_snapshot(), Some(replacement));
        assert!(page.audit_events().is_empty());
        require_approval(&page, "y")?;
        Ok(())
    }

    #[test]
    fn gateway_rejects_old_session_id_after_provider_swap() -> Result<()> {
        if test_bench_support::should_skip_browser_tests() {
            return Ok(());
        }
        let (_first_client, first) = approval_page()?;
        let (_second_client, second) = approval_page()?;
        require_approval(&first, "x")?;
        require_approval(&second, "x")?;
        let active = Arc::new(std::sync::Mutex::new(Arc::clone(&first)));
        let provider_active = Arc::clone(&active);
        let gateway = PageSessionGateway::with_provider(Arc::new(move || {
            Arc::clone(&provider_active.lock().unwrap())
        }));
        let original = gateway.pending_request().unwrap();
        *active.lock().unwrap() = Arc::clone(&second);
        let replacement = second.pending_policy_approval_snapshot().unwrap();
        second.clear_audit_events();
        assert!(gateway.approve(original.id).is_err());
        assert!(gateway.reject(original.id).is_err());
        assert_eq!(second.pending_policy_approval_snapshot(), Some(replacement));
        assert!(second.audit_events().is_empty());
        let current = gateway.pending_request().unwrap();
        assert_ne!(current.id, original.id);
        gateway.reject(current.id)?;
        assert!(second.pending_policy_approval().is_none());
        assert!(first.pending_policy_approval().is_some());
        Ok(())
    }

    #[test]
    fn gateway_does_not_reuse_id_for_unobserved_same_shape_approval_cycle() -> Result<()> {
        if test_bench_support::should_skip_browser_tests() {
            return Ok(());
        }
        let (_client, page) = approval_page()?;
        require_approval(&page, "x")?;
        let gateway = PageSessionGateway::new(Arc::clone(&page));
        let original = gateway.pending_request().unwrap();
        page.reject_pending_policy_action()?;
        require_approval(&page, "x")?;
        let replacement = gateway.pending_request().unwrap();
        assert_eq!(original.target_signature, replacement.target_signature);
        assert_ne!(
            original.id, replacement.id,
            "new approval cycle needs a new identity even without polling None"
        );
        assert!(gateway.approve(original.id).is_err());
        assert!(gateway.reject(original.id).is_err());
        assert!(page.pending_policy_approval().is_some());
        Ok(())
    }

    #[test]
    fn gateway_changes_id_for_scope_and_outcome_only_replacements() -> Result<()> {
        if test_bench_support::should_skip_browser_tests() {
            return Ok(());
        }
        let (_client, page) = approval_page()?;
        let gateway = PageSessionGateway::new(Arc::clone(&page));
        require_approval(&page, "x")?;
        let original = gateway.pending_request().unwrap();
        page.set_policy_rules(vec![approval_rule(ApprovalScope::UntilNavigation, false)])?;
        require_approval(&page, "x")?;
        let scoped = gateway.pending_request().unwrap();
        assert_ne!(original.scope, scoped.scope);
        assert_ne!(original.id, scoped.id);
        page.set_policy_rules(vec![approval_rule(ApprovalScope::UntilNavigation, true)])?;
        require_approval(&page, "x")?;
        let projected = gateway.pending_request().unwrap();
        assert_eq!(scoped.scope, projected.scope);
        assert_ne!(scoped.outcome, projected.outcome);
        assert_ne!(scoped.id, projected.id);
        assert!(gateway.approve(scoped.id).is_err());
        Ok(())
    }

    #[test]
    fn gateway_changes_id_when_navigation_replaces_pending_request() -> Result<()> {
        if test_bench_support::should_skip_browser_tests() {
            return Ok(());
        }
        let (_client, page) = approval_page()?;
        let gateway = PageSessionGateway::new(Arc::clone(&page));
        page.navigate_public("http://127.0.0.1:1/review", true)
            .expect_err("approval precedes network access");
        let original = gateway.pending_request().unwrap();
        page.navigate_public("http://127.0.0.1:1/review", true)
            .expect_err("fresh navigation attempt requires review");
        let replacement = gateway.pending_request().unwrap();
        assert_eq!(original.action, "navigate");
        assert_eq!(original.target_signature, replacement.target_signature);
        assert_ne!(original.id, replacement.id);
        assert!(gateway.approve(original.id).is_err());
        Ok(())
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
                risk_level: core_runtime::RiskLevel::High,
            }),
        }
    }

    fn sample_key() -> RequestKey {
        sample_key_on(&SESSION_A)
    }

    static SESSION_A: std::sync::LazyLock<Arc<()>> = std::sync::LazyLock::new(|| Arc::new(()));

    fn sample_key_on(session: &Arc<()>) -> RequestKey {
        RequestKey {
            session: SessionToken::of(session),
            request_id: Uuid::from_u128(1),
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
            session: SessionToken::of(&SESSION_A),
            request_id: Uuid::from_u128(2),
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
    fn minted_id_cache_peek_returns_none_without_minting_when_nothing_cached() {
        let cache = MintedIdCache::default();
        let key = sample_key();

        assert_eq!(cache.peek(&key), None);
        assert_eq!(
            cache.peek(&key),
            None,
            "peek must not mint an ID as a side effect"
        );
    }

    #[test]
    fn minted_id_cache_peek_returns_the_minted_id_without_minting_a_new_one() {
        let mut cache = MintedIdCache::default();
        let key = sample_key();

        let minted = cache.id_for(&key);

        assert_eq!(cache.peek(&key), Some(minted));
        assert_eq!(cache.peek(&key), Some(minted), "peek must be idempotent");
    }

    #[test]
    fn minted_id_cache_peek_returns_none_for_a_different_key_than_the_one_cached() {
        let mut cache = MintedIdCache::default();
        let cached_key = sample_key();
        let other_key = RequestKey {
            session: SessionToken::of(&SESSION_A),
            request_id: Uuid::from_u128(2),
        };
        cache.id_for(&cached_key);

        assert_eq!(cache.peek(&other_key), None);
    }

    #[test]
    fn minted_id_cache_does_not_reuse_an_id_across_sessions_for_an_identical_request() {
        let mut cache = MintedIdCache::default();
        let session_b = Arc::new(());

        let on_a = cache.id_for(&sample_key_on(&SESSION_A));
        let key_b = sample_key_on(&session_b);

        assert_eq!(
            cache.peek(&key_b),
            None,
            "an ID minted for a crashed session must not validate on its replacement"
        );
        assert_ne!(cache.id_for(&key_b), on_a);
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
