use super::*;

pub(super) fn resolve_policy_target_node<'a>(
    root: &'a SemanticNode,
    target_id: Option<i64>,
    stable_key: Option<&str>,
) -> Option<&'a SemanticNode> {
    if let Some(id) = target_id {
        if let Some(node) = find_node_by_id(root, id) {
            return Some(node);
        }
    }

    if let Some(key) = stable_key {
        if let Some(node) = find_node_by_key(root, key) {
            return Some(node);
        }
    }

    None
}

/// Find the immediate parent node of `target` in the semantic tree rooted at `root`.
pub(super) fn find_parent_of_node<'a>(
    root: &'a SemanticNode,
    target: &SemanticNode,
) -> Option<&'a SemanticNode> {
    for child in &root.children {
        if std::ptr::eq(child as *const _, target as *const _) {
            return Some(root);
        }
        if let Some(found) = find_parent_of_node(child, target) {
            return Some(found);
        }
    }
    None
}

/// Collect surrounding context text for policy evaluation.
///
/// In addition to the target node's own label and attributes, this walks the
/// parent container and collects visible text from sibling nodes so that
/// `context_regex` rules (e.g. matching `Total: $149` outside a button) work
/// correctly for realistic checkout DOM structures.
pub(super) fn policy_context_text(node: &SemanticNode, root: &SemanticNode) -> String {
    let mut parts = Vec::new();

    // Target node: label + attribute values
    push_policy_text_part(&mut parts, node.label.as_deref());
    if let Some(attrs) = &node.attributes {
        for value in attrs.values() {
            push_policy_text_part(&mut parts, Some(value.as_str()));
        }
    }

    // Parent container: label and direct children's text
    if let Some(parent) = find_parent_of_node(root, node) {
        push_policy_text_part(&mut parts, parent.label.as_deref());
        for sibling in &parent.children {
            // Collect text-role siblings and their descendant text
            collect_policy_descendant_text(sibling, &mut parts);
        }
    }

    parts.join(" ")
}

pub(super) fn policy_target_text(node: &SemanticNode) -> Option<String> {
    let mut parts = Vec::new();
    push_policy_text_part(&mut parts, node.label.as_deref());
    collect_policy_descendant_text(node, &mut parts);

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

pub(super) fn collect_policy_descendant_text(node: &SemanticNode, parts: &mut Vec<String>) {
    for child in &node.children {
        if child.role == "text" {
            push_policy_text_part(parts, child.label.as_deref());
        }
        collect_policy_descendant_text(child, parts);
    }
}

pub(super) fn push_policy_text_part(parts: &mut Vec<String>, value: Option<&str>) {
    let Some(normalized) = value
        .map(str::trim)
        .filter(|candidate| !candidate.is_empty())
    else {
        return;
    };

    if parts.iter().any(|existing| existing == normalized) {
        return;
    }

    parts.push(normalized.to_string());
}

pub(super) type NavigationRequestInterceptor =
    dyn headless_chrome::browser::tab::RequestInterceptor + Send + Sync;

pub(super) trait NavigationInterceptionControl: Send + Sync {
    fn install_interceptor(&self, interceptor: Arc<NavigationRequestInterceptor>) -> Result<()>;
    fn enable_fetch(
        &self,
        patterns: &[headless_chrome::protocol::cdp::Fetch::RequestPattern],
    ) -> Result<()>;
    fn restore_default_interceptor(&self) -> Result<()>;
    fn disable_fetch(&self) -> Result<()>;
}

pub(super) struct TabNavigationInterceptionControl {
    pub(super) tab: Arc<headless_chrome::Tab>,
}

impl NavigationInterceptionControl for TabNavigationInterceptionControl {
    fn install_interceptor(&self, interceptor: Arc<NavigationRequestInterceptor>) -> Result<()> {
        self.tab.enable_request_interception(interceptor)
    }

    fn enable_fetch(
        &self,
        patterns: &[headless_chrome::protocol::cdp::Fetch::RequestPattern],
    ) -> Result<()> {
        self.tab.enable_fetch(Some(patterns), None).map(|_| ())
    }

    fn restore_default_interceptor(&self) -> Result<()> {
        self.tab.enable_request_interception(Arc::new(
            |_: Arc<headless_chrome::browser::transport::Transport>,
             _: headless_chrome::browser::transport::SessionId,
             _: headless_chrome::protocol::cdp::Fetch::events::RequestPausedEvent| {
                headless_chrome::browser::tab::RequestPausedDecision::Continue(None)
            },
        ))
    }

    fn disable_fetch(&self) -> Result<()> {
        self.tab.disable_fetch().map(|_| ())
    }
}

pub(super) struct NavigationInterceptionGuard {
    pub(super) control: Arc<dyn NavigationInterceptionControl>,
    pub(super) active: bool,
}

impl NavigationInterceptionGuard {
    pub(super) fn install<F>(
        tab: Arc<headless_chrome::Tab>,
        interceptor: Arc<F>,
        patterns: &[headless_chrome::protocol::cdp::Fetch::RequestPattern],
    ) -> Result<Self>
    where
        F: headless_chrome::browser::tab::RequestInterceptor + Send + Sync + 'static,
    {
        Self::install_with_control(
            Arc::new(TabNavigationInterceptionControl { tab }),
            interceptor,
            patterns,
        )
    }

    pub(super) fn install_with_control<F>(
        control: Arc<dyn NavigationInterceptionControl>,
        interceptor: Arc<F>,
        patterns: &[headless_chrome::protocol::cdp::Fetch::RequestPattern],
    ) -> Result<Self>
    where
        F: headless_chrome::browser::tab::RequestInterceptor + Send + Sync + 'static,
    {
        let guard = Self {
            control,
            active: true,
        };
        let interceptor: Arc<NavigationRequestInterceptor> = interceptor;
        guard
            .control
            .install_interceptor(interceptor)
            .context("failed to install public navigation interceptor")?;
        guard
            .control
            .enable_fetch(patterns)
            .context("failed to enable public navigation interception")?;
        Ok(guard)
    }

    pub(super) fn finish(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        let restore_result = self
            .control
            .restore_default_interceptor()
            .context("failed to restore default request interceptor");
        let disable_result = self
            .control
            .disable_fetch()
            .context("failed to disable public navigation interception");
        restore_result.and(disable_result)
    }
}

impl Drop for NavigationInterceptionGuard {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

pub(super) fn evaluate_navigation_destination(
    context: &NavigationPolicyContext,
    destination: &ValidatedNavigationUrl,
    approval_signature: &str,
    clear_pending_on_block: bool,
) -> std::result::Result<(), NavigationPolicyFailure> {
    let decision = context
        .policy_engine
        .lock()
        .map_err(|_| NavigationPolicyFailure::Rejected {
            message: "navigation policy engine is unavailable".to_string(),
        })?
        .evaluate(&PolicyContext {
            url: destination.canonical_url().to_string(),
            action: "navigate".to_string(),
            target_role: None,
            target_text: None,
            surrounding_text: None,
        });
    let rule_id = decision
        .rule_id
        .clone()
        .unwrap_or_else(|| "unnamed-policy-rule".to_string());
    context.audit_logger.log(AuditEvent::PolicyDecision {
        rule_id: rule_id.clone(),
        action: "navigate".to_string(),
        decision: match decision.action {
            PolicyAction::Allow => "allow",
            PolicyAction::Block => "block",
            PolicyAction::RequireHumanApproval => "require_human_approval",
        }
        .to_string(),
        destination_fingerprint: Some(format!("sha256:{}", destination.destination_digest())),
        timestamp: epoch_millis_u64(),
    });

    let run_plugins = || {
        let intent = serde_json::json!({
            "action": "navigate",
            "url": destination.canonical_url(),
        })
        .to_string();
        let (outcome, events) = run_policy_hooks(
            &intent,
            context
                .plugin_hooks
                .policy_plugins
                .iter()
                .map(|p| p.as_ref()),
        );
        // Plugin reasons are untrusted and may echo the full destination URL.
        // Keep attribution and allow/block result, but never persist the reason.
        for event in events {
            if let AuditEvent::PluginPolicyDecision {
                plugin_id,
                allowed,
                timestamp,
                ..
            } = event
            {
                context.audit_logger.log(AuditEvent::PluginPolicyDecision {
                    plugin_id,
                    allowed,
                    reason: None,
                    timestamp,
                });
            }
        }
        match outcome {
            PolicyHookOutcome::Allow => Ok(()),
            PolicyHookOutcome::Block { plugin_id, .. } => Err(NavigationPolicyFailure::Blocked {
                rule_id: format!("plugin:{plugin_id}"),
            }),
        }
    };

    match decision.action {
        PolicyAction::Allow => run_plugins(),
        PolicyAction::Block => {
            if clear_pending_on_block {
                if let Ok(mut approvals) = context.policy_approvals.lock() {
                    approvals.pending = None;
                }
            }
            Err(NavigationPolicyFailure::Blocked { rule_id })
        }
        PolicyAction::RequireHumanApproval => {
            let scope = decision.scope.unwrap_or(ApprovalScope::ActionOnly);
            if consume_navigation_approval(context, &rule_id, scope, approval_signature) {
                return run_plugins();
            }
            let request = PolicyApprovalRequest {
                rule_id: rule_id.clone(),
                scope,
                action: "navigate".to_string(),
                target_signature: approval_signature.to_string(),
                outcome: decision.outcome.clone(),
            };
            context
                .policy_approvals
                .lock()
                .map_err(|_| NavigationPolicyFailure::Rejected {
                    message: "navigation approval state is unavailable".to_string(),
                })?
                .pending = Some(request);
            context.audit_logger.log(AuditEvent::HitlEvent {
                event_type: "request".to_string(),
                reason: Some(format!("Requires human approval for rule {rule_id}")),
                user_id: None,
                timestamp: epoch_millis_u64(),
            });
            Err(NavigationPolicyFailure::HumanApprovalRequired {
                rule_id,
                scope,
                outcome: decision.outcome,
            })
        }
    }
}

pub(super) fn consume_navigation_approval(
    context: &NavigationPolicyContext,
    rule_id: &str,
    scope: ApprovalScope,
    approval_signature: &str,
) -> bool {
    let now_ms = epoch_millis();
    let navigation_epoch = context.navigation_epoch.load(Ordering::Relaxed);
    let Ok(mut approvals) = context.policy_approvals.lock() else {
        return false;
    };
    approvals.granted.retain(|grant| {
        is_grant_valid(
            grant,
            navigation_epoch,
            &context.approval_context_url,
            now_ms,
        )
    });
    let Some(index) = approvals.granted.iter().position(|grant| {
        grant.request.rule_id == rule_id
            && grant.request.scope == scope
            && grant.request.action == "navigate"
            && grant.request.target_signature == approval_signature
    }) else {
        return false;
    };
    if scope == ApprovalScope::ActionOnly {
        approvals.granted.remove(index);
    }
    true
}

pub(super) fn policy_target_signature(
    node: Option<&SemanticNode>,
    target_id: Option<i64>,
    stable_key: Option<&str>,
) -> String {
    if let Some(key) = node.and_then(|resolved| resolved.stable_key.as_deref()) {
        return key.to_string();
    }

    if let Some(key) = stable_key {
        let normalized = key.trim();
        if !normalized.is_empty() {
            return normalized.to_string();
        }
    }

    if let Some(id) = node.map(|resolved| resolved.backend_node_id).or(target_id) {
        return format!("backend_node_id:{id}");
    }

    "unknown-target".to_string()
}

pub(super) fn is_grant_valid(
    grant: &GrantedPolicyApproval,
    navigation_epoch: u64,
    current_url: &str,
    now_ms: u128,
) -> bool {
    match grant.request.scope {
        ApprovalScope::ActionOnly => {
            grant.granted_navigation_epoch == navigation_epoch
                && grant.remaining_uses.unwrap_or(0) > 0
        }
        // UntilNavigation expires when either the explicit navigate() increments the epoch
        // OR the URL changed due to a click-driven navigation (which does not increment epoch).
        ApprovalScope::UntilNavigation => {
            grant.granted_navigation_epoch == navigation_epoch && grant.granted_url == current_url
        }
        ApprovalScope::Timeboxed { .. } => grant
            .expires_at_epoch_ms
            .is_some_and(|expires_at| now_ms <= expires_at),
    }
}
