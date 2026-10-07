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
        self.policy_approvals
            .lock()
            .ok()
            .and_then(|state| state.pending.clone())
    }

    /// Approve the latest pending policy request and return exactly the request that was
    /// approved, or `None` when nothing is pending.
    ///
    /// Callers that report on or log the approval should use the returned request instead of
    /// reading [`pending_policy_approval`](Self::pending_policy_approval) first: between the two
    /// calls the pending request could be replaced, so the report would describe a different
    /// request from the one that was granted.
    pub fn approve_pending_policy_request(&self) -> Result<Option<PolicyApprovalRequest>> {
        let guard = self
            .policy_approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock policy approval state"))?;

        let Some(request) = guard.pending.clone() else {
            return Ok(None);
        };

        // Drop the lock before calling current_url() to avoid potential deadlock.
        drop(guard);

        let granted_navigation_epoch = self.navigation_epoch.load(Ordering::Relaxed);
        let granted_url = self.current_url().unwrap_or_default();
        let now_ms = epoch_millis();
        let (expires_at_epoch_ms, remaining_uses) = match request.scope {
            ApprovalScope::ActionOnly => (None, Some(1)),
            ApprovalScope::UntilNavigation => (None, None),
            ApprovalScope::Timeboxed { ms } => (Some(now_ms + u128::from(ms)), None),
        };

        let mut guard = self
            .policy_approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock policy approval state"))?;
        if guard.pending.as_ref() != Some(&request) {
            anyhow::bail!("Pending policy approval request changed before commit");
        }
        guard.pending = None;
        guard.granted.push(GrantedPolicyApproval {
            request: request.clone(),
            granted_navigation_epoch,
            granted_url,
            expires_at_epoch_ms,
            remaining_uses,
        });

        Ok(Some(request))
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
        let mut guard = self
            .policy_approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock policy approval state"))?;

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
        let captured = self.capture_state(LoadProfile::Interactive)?;
        let target_node = resolve_policy_target_node(captured.root(), target_id, stable_key);

        let target_role = target_node.map(|node| node.role.clone());
        let target_text = target_node.and_then(policy_target_text);
        let surrounding_text = target_node.map(|node| policy_context_text(node, captured.root()));
        let target_signature = policy_target_signature(target_node, target_id, stable_key);
        let url = self.current_url()?;

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
                    &url,
                ) {
                    // Human approval was granted — still run plugin policy hooks so
                    // plugins have a chance to veto even pre-approved actions.
                    self.enforce_plugin_policy(action, &url)?;
                    return Ok(());
                }

                self.set_pending_policy_approval(PolicyApprovalRequest {
                    rule_id: rule_id.clone(),
                    scope,
                    action: action.to_string(),
                    target_signature,
                    outcome: decision.outcome.clone(),
                })?;

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

    pub(super) fn set_pending_policy_approval(&self, request: PolicyApprovalRequest) -> Result<()> {
        let mut guard = self
            .policy_approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock policy approval state"))?;
        guard.pending = Some(request);
        Ok(())
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
        current_url: &str,
    ) -> bool {
        let now_ms = epoch_millis();
        let navigation_epoch = self.navigation_epoch.load(Ordering::Relaxed);

        let Ok(mut guard) = self.policy_approvals.lock() else {
            return false;
        };

        guard
            .granted
            .retain(|grant| is_grant_valid(grant, navigation_epoch, current_url, now_ms));

        let Some(index) = guard.granted.iter().position(|grant| {
            grant.request.rule_id == rule_id
                && grant.request.scope == scope
                && grant.request.action == action
                && grant.request.target_signature == target_signature
        }) else {
            return false;
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

        true
    }
}
