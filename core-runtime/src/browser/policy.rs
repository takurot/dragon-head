use super::*;

impl PageSession {
    /// Returns the currently configured policy rules for this page session.
    ///
    /// Used to reapply policy configuration to the fresh `PageSession`
    /// created by [`BrowserClient::relaunch`] (ISSUE-149).
    pub fn policy_rules(&self) -> Result<Vec<PolicyRule>> {
        let guard = self
            .policy_engine
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock policy engine"))?;
        Ok(guard.rules().to_vec())
    }

    /// Replace policy rules for this page session.
    pub fn set_policy_rules(&self, rules: Vec<PolicyRule>) -> Result<()> {
        let engine = PolicyEngine::try_new(rules)?;
        let mut engine_guard = self
            .policy_engine
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock policy engine"))?;
        *engine_guard = engine;
        drop(engine_guard);

        let mut approvals_guard = self
            .policy_approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock policy approval state"))?;
        approvals_guard.pending = None;
        approvals_guard.granted.clear();
        Ok(())
    }

    /// Returns the current pending human-approval request, if any.
    pub fn pending_policy_approval(&self) -> Option<PolicyApprovalRequest> {
        self.pending_policy_approval_snapshot()
            .map(|snapshot| snapshot.request)
    }

    /// Returns the pending request with the identity of its exact approval cycle.
    /// Bridges must pass this identity back when applying a human's decision.
    pub fn pending_policy_approval_snapshot(&self) -> Option<PolicyApprovalRequestSnapshot> {
        self.policy_approvals.lock().ok().and_then(|state| {
            state
                .pending
                .as_ref()
                .map(|pending| PolicyApprovalRequestSnapshot {
                    id: pending.id,
                    request: pending.request.clone(),
                })
        })
    }

    /// Approve the latest pending policy request and return exactly the request that was
    /// approved, or `None` when nothing is pending.
    ///
    /// Callers that report on or log the approval should use the returned request instead of
    /// reading [`pending_policy_approval`](Self::pending_policy_approval) first: between the two
    /// calls the pending request could be replaced, so the report would describe a different
    /// request from the one that was granted.
    pub fn approve_pending_policy_request(&self) -> Result<Option<PolicyApprovalRequest>> {
        self.approve_pending_policy_request_via("api")
    }

    /// Like [`approve_pending_policy_request`](Self::approve_pending_policy_request), recording
    /// `channel` (e.g. `ask_human`, `hitl_bridge`) in the `HITL_EVENT` `approved` audit entry so
    /// a reviewer can tell which path granted the approval (ISSUE-372).
    pub fn approve_pending_policy_request_via(
        &self,
        channel: &str,
    ) -> Result<Option<PolicyApprovalRequest>> {
        self.approve_pending_policy_request_with_expected(None, channel)
    }

    /// Approve only the identified pending cycle. A stale identity never grants or audits
    /// a replacement request, including one with identical review data.
    pub fn approve_pending_policy_request_if_current(
        &self,
        expected_id: uuid::Uuid,
        channel: &str,
    ) -> Result<PolicyApprovalRequest> {
        self.approve_pending_policy_request_with_expected(Some(expected_id), channel)?
            .ok_or_else(|| anyhow::anyhow!("No pending policy approval request"))
    }

    fn approve_pending_policy_request_with_expected(
        &self,
        expected_id: Option<uuid::Uuid>,
        channel: &str,
    ) -> Result<Option<PolicyApprovalRequest>> {
        let guard = self
            .policy_approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock policy approval state"))?;

        let Some(snapshot) = guard.pending.clone() else {
            if expected_id.is_some() {
                anyhow::bail!("Expected policy approval request is no longer pending");
            }
            return Ok(None);
        };
        if expected_id.is_some_and(|expected| expected != snapshot.id) {
            anyhow::bail!("Expected policy approval request is no longer pending");
        }
        // All browser I/O precedes both observation and approval-state locks.
        drop(guard);
        let document = self.approval_document_snapshot()?;
        self.commit_pending_policy_approval(snapshot, &document, channel)
            .map(Some)
    }

    fn commit_pending_policy_approval(
        &self,
        pending: PendingPolicyApproval,
        document: &DocumentSnapshot,
        channel: &str,
    ) -> Result<PolicyApprovalRequest> {
        if let ApprovalBinding::PageAction(reviewed) = &pending.binding {
            let current = PageApprovalContext::from_snapshot(document)?;
            if !reviewed.matches(&current, pending.request.scope) {
                anyhow::bail!("Policy approval document changed since the request was created");
            }
        }
        self.with_approval_document_context_current(document, || {
            let mut guard = self
                .policy_approvals
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock policy approval state"))?;
            if guard.pending.as_ref().map(|request| request.id) != Some(pending.id) {
                anyhow::bail!("Pending policy approval request changed before commit");
            }
            let request = pending.request;
            let (expires_at_epoch_ms, remaining_uses) = match request.scope {
                ApprovalScope::ActionOnly => (None, Some(1)),
                ApprovalScope::UntilNavigation => (None, None),
                ApprovalScope::Timeboxed { ms } => (Some(epoch_millis() + u128::from(ms)), None),
            };
            guard.pending = None;
            guard.granted.push(GrantedPolicyApproval {
                request: request.clone(),
                binding: pending.binding,
                granted_navigation_epoch: self.navigation_epoch.load(Ordering::Acquire),
                granted_url_fingerprint: approval_url_fingerprint(&document.url),
                expires_at_epoch_ms,
                remaining_uses,
            });
            // The observation -> approval lock order serializes document changes and grant
            // publication. Audit remains before another action can consume this grant.
            self.audit_logger.log(AuditEvent::HitlEvent {
                event_type: "approved".to_string(),
                reason: Some(format!(
                    "Approved via {channel}: rule {} scope {:?} action {} target {}",
                    request.rule_id, request.scope, request.action, request.target_signature
                )),
                user_id: None,
                timestamp: epoch_millis_u64(),
            });
            Ok(request)
        })
    }

    /// Approve the latest pending policy request.
    ///
    /// Errors when nothing is pending; see
    /// [`approve_pending_policy_request`](Self::approve_pending_policy_request) to also get the
    /// approved request back.
    pub fn approve_pending_policy_action(&self) -> Result<()> {
        self.approve_pending_policy_request()?
            .map(|_| ())
            .ok_or_else(|| anyhow::anyhow!("No pending policy approval request"))
    }

    /// Reject the latest pending policy request.
    ///
    /// Unlike [`approve_pending_policy_action`], the request is discarded rather
    /// than recorded as granted — the originating action remains blocked. Used by
    /// HITL bridges (e.g. Slack/Teams reference implementations) to relay a human's
    /// "Reject" decision back into the session.
    ///
    /// [`approve_pending_policy_action`]: Self::approve_pending_policy_action
    pub fn reject_pending_policy_action(&self) -> Result<()> {
        self.reject_pending_policy_request_with_expected(None)
    }

    /// Reject only the identified pending approval cycle; a replacement stays pending.
    pub fn reject_pending_policy_request_if_current(&self, expected_id: uuid::Uuid) -> Result<()> {
        self.reject_pending_policy_request_with_expected(Some(expected_id))
    }

    fn reject_pending_policy_request_with_expected(
        &self,
        expected_id: Option<uuid::Uuid>,
    ) -> Result<()> {
        let mut guard = self
            .policy_approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock policy approval state"))?;

        if expected_id.is_some() && guard.pending.as_ref().map(|pending| pending.id) != expected_id
        {
            anyhow::bail!("Expected policy approval request is no longer pending");
        }
        if guard.pending.take().is_none() {
            anyhow::bail!("No pending policy approval request");
        }
        drop(guard);

        self.audit_logger.log(AuditEvent::HitlEvent {
            event_type: "rejected".to_string(),
            reason: None,
            user_id: None,
            timestamp: epoch_millis_u64(),
        });

        Ok(())
    }

    pub(super) fn enforce_policy(
        &self,
        target_id: Option<i64>,
        stable_key: Option<&str>,
        action: &str,
    ) -> Result<()> {
        let document = self
            .live_document_snapshot()
            .map_err(|_| anyhow::anyhow!("Could not read the policy target document"))?;
        let reviewed = PageApprovalContext::from_snapshot(&document)?;
        let captured = self.capture_state(LoadProfile::Interactive)?;
        let target_node = resolve_policy_target_node(captured.root(), target_id, stable_key);

        let target_role = target_node.map(|node| node.role.clone());
        let target_text = target_node.and_then(policy_target_text);
        let surrounding_text = target_node.map(|node| policy_context_text(node, captured.root()));
        let target_signature = policy_target_signature(target_node, target_id, stable_key);
        let after_capture = self
            .live_document_snapshot()
            .map_err(|_| anyhow::anyhow!("Could not read the policy target document"))?;
        if !reviewed.same_document(&PageApprovalContext::from_snapshot(&after_capture)?) {
            anyhow::bail!("Policy target document changed during state capture");
        }
        let url = document.url.clone();

        let decision = {
            let guard = self
                .policy_engine
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock policy engine"))?;
            guard.evaluate(&PolicyContext {
                url: url.clone(),
                action: action.to_string(),
                target_role,
                target_text,
                surrounding_text,
            })
        };

        self.audit_logger.log(AuditEvent::PolicyDecision {
            rule_id: decision
                .rule_id
                .clone()
                .unwrap_or_else(|| "unnamed-policy-rule".to_string()),
            action: action.to_string(),
            decision: match decision.action {
                PolicyAction::Allow => "allow".to_string(),
                PolicyAction::Block => "block".to_string(),
                PolicyAction::RequireHumanApproval => "require_human_approval".to_string(),
            },
            destination_fingerprint: None,
            timestamp: epoch_millis_u64(),
        });

        match decision.action {
            PolicyAction::Allow => {
                // Built-in policy allows — now run plugin policy hooks.
                self.enforce_plugin_policy(action, &url)?;
                Ok(())
            }
            PolicyAction::Block => Err(ActionError::Blocked {
                rule_id: decision
                    .rule_id
                    .unwrap_or_else(|| "unnamed-policy-rule".to_string()),
            }
            .into()),
            PolicyAction::RequireHumanApproval => {
                let rule_id = decision
                    .rule_id
                    .unwrap_or_else(|| "unnamed-policy-rule".to_string());
                let scope = decision.scope.unwrap_or(ApprovalScope::ActionOnly);

                if self.consume_granted_policy_approval(
                    &rule_id,
                    scope,
                    action,
                    &target_signature,
                    &document,
                )? {
                    // Human approval was granted — still run plugin policy hooks so
                    // plugins have a chance to veto even pre-approved actions.
                    self.enforce_plugin_policy(action, &url)?;
                    return Ok(());
                }

                self.set_pending_policy_approval_for_document(
                    PolicyApprovalRequest {
                        rule_id: rule_id.clone(),
                        scope,
                        action: action.to_string(),
                        target_signature,
                        outcome: decision.outcome.clone(),
                    },
                    &document,
                )?;

                self.audit_logger.log(AuditEvent::HitlEvent {
                    event_type: "request".to_string(),
                    reason: Some(format!("Requires human approval for rule {}", rule_id)),
                    user_id: None,
                    timestamp: epoch_millis_u64(),
                });

                Err(ActionError::HumanApprovalRequired {
                    rule_id,
                    scope,
                    outcome: decision.outcome,
                }
                .into())
            }
        }
    }

    /// Run all policy plugin hooks for the given action and URL.
    ///
    /// If any plugin blocks (or fails), the action is rejected (fail-closed).
    /// All plugin decisions are recorded in the audit log.
    pub(super) fn enforce_plugin_policy(&self, action: &str, url: &str) -> Result<()> {
        let plugins = &self.plugin_hooks.policy_plugins;
        if plugins.is_empty() {
            return Ok(());
        }

        let intent_json = serde_json::json!({
            "action": action,
            "url": url,
        })
        .to_string();

        let (outcome, events) = run_policy_hooks(&intent_json, plugins.iter().map(|p| p.as_ref()));

        for event in events {
            self.audit_logger.log(event);
        }

        match outcome {
            PolicyHookOutcome::Allow => Ok(()),
            PolicyHookOutcome::Block { plugin_id, reason } => {
                let rule_id = format!("plugin:{plugin_id}");
                let reason_str = reason.as_deref().unwrap_or("blocked by plugin");
                tracing::warn!(
                    plugin_id,
                    reason = reason_str,
                    "action blocked by policy plugin"
                );
                Err(ActionError::Blocked { rule_id }.into())
            }
        }
    }

    fn set_pending_policy_approval_for_document(
        &self,
        request: PolicyApprovalRequest,
        document: &DocumentSnapshot,
    ) -> Result<()> {
        let current = self.approval_document_snapshot()?;
        let reviewed = PageApprovalContext::from_snapshot(document)?;
        if !reviewed.matches(
            &PageApprovalContext::from_snapshot(&current)?,
            request.scope,
        ) {
            anyhow::bail!("Policy target document changed before request publication");
        }
        self.with_approval_document_context_current(&current, || {
            let mut guard = self
                .policy_approvals
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock policy approval state"))?;
            guard.replace_pending(request, ApprovalBinding::PageAction(reviewed));
            Ok(())
        })
    }

    pub(super) fn clear_pending_policy_approval(&self) {
        if let Ok(mut guard) = self.policy_approvals.lock() {
            guard.pending = None;
        }
    }

    pub(super) fn consume_granted_policy_approval(
        &self,
        rule_id: &str,
        scope: ApprovalScope,
        action: &str,
        target_signature: &str,
        document: &DocumentSnapshot,
    ) -> Result<bool> {
        let current = self.approval_document_snapshot()?;
        let reviewed = PageApprovalContext::from_snapshot(document)?;
        let current_binding = PageApprovalContext::from_snapshot(&current)?;
        if !reviewed.matches(&current_binding, scope) {
            anyhow::bail!("Policy target document changed before approval consumption");
        }
        let now_ms = epoch_millis();
        let navigation_epoch = self.navigation_epoch.load(Ordering::Relaxed);
        self.with_approval_document_context_current(&current, || {
            let mut guard = self
                .policy_approvals
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock policy approval state"))?;
            guard.granted.retain(|grant| {
                let context_valid = match &grant.binding {
                    ApprovalBinding::PageAction(context) => {
                        context.matches(&current_binding, grant.request.scope)
                    }
                    ApprovalBinding::NavigationCapability => true,
                };
                context_valid && is_grant_valid(grant, navigation_epoch, &current.url, now_ms)
            });

            let Some(index) = guard.granted.iter().position(|grant| {
                matches!(grant.binding, ApprovalBinding::PageAction(_))
                    && grant.request.rule_id == rule_id
                    && grant.request.scope == scope
                    && grant.request.action == action
                    && grant.request.target_signature == target_signature
            }) else {
                return Ok(false);
            };

            match scope {
                ApprovalScope::ActionOnly => {
                    let mut should_remove = true;
                    if let Some(remaining) = guard.granted[index].remaining_uses.as_mut() {
                        if *remaining > 1 {
                            *remaining -= 1;
                            should_remove = false;
                        }
                    }

                    if should_remove {
                        guard.granted.remove(index);
                    }
                }
                ApprovalScope::UntilNavigation | ApprovalScope::Timeboxed { .. } => {}
            }

            Ok(true)
        })
    }
}

#[cfg(test)]
mod approval_identity_tests {
    use super::*;

    fn request() -> PolicyApprovalRequest {
        PolicyApprovalRequest {
            rule_id: "review".to_string(),
            scope: ApprovalScope::ActionOnly,
            action: "click".to_string(),
            target_signature: "same-target".to_string(),
            outcome: None,
        }
    }

    fn binding() -> ApprovalBinding {
        ApprovalBinding::PageAction(
            PageApprovalContext::from_snapshot(&DocumentSnapshot {
                frame_id: "frame".to_string(),
                loader_id: "document".to_string(),
                url: "https://example.com/".to_string(),
                revision: 1,
            })
            .unwrap(),
        )
    }

    #[test]
    fn pending_replacement_always_receives_a_new_identity() {
        let approvals = Mutex::new(PolicyApprovalState::default());
        let mut guard = approvals.lock().unwrap();
        guard.replace_pending(request(), binding());
        let original = guard.pending.clone().unwrap();
        assert_eq!(guard.pending.as_ref().unwrap().id, original.id);
        guard.replace_pending(request(), binding());
        let replacement = guard.pending.as_ref().unwrap();
        assert_eq!(original.request, replacement.request);
        assert_ne!(original.id, replacement.id);
        assert!(guard.granted.is_empty());
    }

    #[test]
    fn clearing_and_recreating_identical_request_changes_identity() {
        let approvals = Mutex::new(PolicyApprovalState::default());
        let mut guard = approvals.lock().unwrap();
        guard.replace_pending(request(), binding());
        let original_id = guard.pending.as_ref().unwrap().id;
        guard.pending = None;
        guard.replace_pending(request(), binding());
        assert_ne!(guard.pending.as_ref().unwrap().id, original_id);
        assert!(guard.granted.is_empty());
    }

    #[test]
    fn navigation_between_context_capture_and_grant_commit_never_publishes_a_grant() -> Result<()> {
        if test_bench_support::should_skip_browser_tests() {
            return Ok(());
        }
        let client = BrowserClient::new()?;
        let page = client.new_page()?;
        page.navigate("data:text/html,<button>Delete</button>")?;
        let document = page.approval_document_snapshot()?;
        page.set_pending_policy_approval_for_document(request(), &document)?;
        let pending = page
            .policy_approvals
            .lock()
            .unwrap()
            .pending
            .clone()
            .unwrap();
        // Raw trusted CDP navigation keeps the pending UUID unchanged, unlike navigate().
        page.inner
            .navigate_to("data:text/html,<button>Different</button>")?
            .wait_until_navigated()?;
        page.live_document_snapshot()?;
        page.clear_audit_events();
        assert!(page
            .commit_pending_policy_approval(pending.clone(), &document, "test")
            .is_err());
        let approvals = page.policy_approvals.lock().unwrap();
        assert_eq!(
            approvals.pending.as_ref().map(|request| request.id),
            Some(pending.id)
        );
        assert!(approvals.granted.is_empty());
        assert!(!page.audit_events().iter().any(|event| matches!(event,
            AuditEvent::HitlEvent { event_type, .. } if event_type == "approved")));
        Ok(())
    }

    #[test]
    fn stale_semantic_target_cannot_be_published_for_a_new_document() -> Result<()> {
        if test_bench_support::should_skip_browser_tests() {
            return Ok(());
        }
        let client = BrowserClient::new()?;
        let page = client.new_page()?;
        page.navigate("data:text/html,<button>Delete</button>")?;
        let captured_target_document = page.approval_document_snapshot()?;
        page.inner
            .navigate_to("data:text/html,<button>Different</button>")?
            .wait_until_navigated()?;
        assert!(page
            .set_pending_policy_approval_for_document(request(), &captured_target_document)
            .is_err());
        assert!(page.pending_policy_approval_snapshot().is_none());
        Ok(())
    }

    #[test]
    fn document_read_failure_never_falls_back_to_an_empty_url_grant() -> Result<()> {
        if test_bench_support::should_skip_browser_tests() {
            return Ok(());
        }
        let client = BrowserClient::new()?;
        let page = client.new_page()?;
        page.navigate("data:text/html,private-content")?;
        let document = page.approval_document_snapshot()?;
        page.set_pending_policy_approval_for_document(request(), &document)?;
        drop(client);
        page.clear_audit_events();
        let error = page.approve_pending_policy_action().unwrap_err();
        assert!(!format!("{error:#}").contains("private-content"));
        assert!(page.policy_approvals.lock().unwrap().granted.is_empty());
        assert!(!page.audit_events().iter().any(|event| matches!(event,
            AuditEvent::HitlEvent { event_type, .. } if event_type == "approved")));
        Ok(())
    }
}
