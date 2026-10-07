use super::*;

impl PageSession {
    pub fn navigate(&self, url: &str) -> Result<()> {
        let previous_url = self.current_url().ok();
        self.inner.navigate_to(url).context("Failed to navigate")?;
        if let Err(wait_error) = self.inner.wait_until_navigated() {
            self.wait_for_navigation_fallback(url, previous_url.as_deref())
                .with_context(|| {
                    format!("Failed to wait for navigation (primary wait error: {wait_error})")
                })?;
        }
        self.clear_stable_key_index();
        self.clear_semantic_capture_cache();
        self.navigation_epoch.fetch_add(1, Ordering::Relaxed);
        self.clear_pending_policy_approval();
        Ok(())
    }

    /// Navigate an untrusted public MCP destination through URL/network validation,
    /// destination-aware policy evaluation, and top-level redirect interception.
    ///
    /// Unlike [`navigate`](Self::navigate), this boundary accepts only HTTP(S).
    /// The internal method intentionally retains support for `data:` fixtures.
    pub fn navigate_public(&self, url: &str, allow_private_network: bool) -> Result<String> {
        use headless_chrome::{
            browser::{
                tab::RequestPausedDecision,
                transport::{SessionId, Transport},
            },
            protocol::cdp::{
                Fetch::{self, FailRequest, RequestPattern, RequestStage},
                Network::{ErrorReason, ResourceType},
                Page,
            },
        };

        let _serialized = self
            .public_navigation_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("public navigation lock is unavailable"))?;
        let network_policy = if allow_private_network {
            NavigationNetworkPolicy::AllowPrivate
        } else {
            NavigationNetworkPolicy::PublicOnly
        };
        let requested = validate_public_navigation_url(url, network_policy)?;
        self.audit_logger.log(AuditEvent::ToolCall {
            tool_name: "navigate".to_string(),
            args: serde_json::json!({ "destination": requested.sanitized_projection() }),
            timestamp: epoch_millis_u64(),
        });

        let approval_context_url = self.current_url().unwrap_or_default();
        let policy_context = Arc::new(NavigationPolicyContext {
            policy_engine: Arc::clone(&self.policy_engine),
            policy_approvals: Arc::clone(&self.policy_approvals),
            navigation_epoch: Arc::clone(&self.navigation_epoch),
            audit_logger: Arc::clone(&self.audit_logger),
            plugin_hooks: Arc::clone(&self.plugin_hooks),
            approval_context_url,
        });
        evaluate_navigation_destination(
            &policy_context,
            &requested,
            requested.destination_digest(),
            false,
        )
        .map_err(NavigationPolicyFailure::into_error)?;

        let frame_tree = self
            .inner
            .call_method(Page::GetFrameTree(None))
            .context("failed to inspect the main frame before public navigation")?;
        let main_frame_id = frame_tree.frame_tree.frame.id;
        let interception_state = Arc::new(Mutex::new(RedirectInterceptionState {
            main_frame_id,
            initial_url: requested.canonical_url().to_string(),
            initial_request_seen: false,
            original_url: requested.canonical_url().to_string(),
            network_policy,
            request_chain: HashSet::new(),
            evaluated_destinations: HashSet::from([requested.destination_digest().to_string()]),
            failure: None,
        }));

        let callback_state = Arc::clone(&interception_state);
        let callback_policy = Arc::clone(&policy_context);
        let interceptor = Arc::new(
            move |_transport: Arc<Transport>,
                  _session_id: SessionId,
                  event: Fetch::events::RequestPausedEvent| {
                let mut state = match callback_state.lock() {
                    Ok(state) => state,
                    Err(_) => {
                        return RequestPausedDecision::Fail(FailRequest {
                            request_id: event.params.request_id,
                            error_reason: ErrorReason::BlockedByClient,
                        })
                    }
                };
                if event.params.frame_id != state.main_frame_id
                    || event.params.resource_Type != ResourceType::Document
                {
                    return RequestPausedDecision::Continue(None);
                }
                if state.failure.is_some() {
                    return RequestPausedDecision::Fail(FailRequest {
                        request_id: event.params.request_id,
                        error_reason: ErrorReason::BlockedByClient,
                    });
                }

                let request_id = event.params.request_id.clone();
                if let Some(parent_request_id) = event.params.redirected_request_id.as_ref() {
                    if !state.request_chain.contains(parent_request_id) {
                        return RequestPausedDecision::Continue(None);
                    }
                } else {
                    let canonical = validate_public_navigation_url_with(
                        &event.params.request.url,
                        NavigationNetworkPolicy::AllowPrivate,
                        |_, _| Ok(Vec::new()),
                    );
                    if !state.initial_request_seen
                        && canonical
                            .as_ref()
                            .is_ok_and(|candidate| candidate.canonical_url() == state.initial_url)
                    {
                        state.initial_request_seen = true;
                        state.request_chain.insert(request_id);
                        return RequestPausedDecision::Continue(None);
                    }
                }

                let destination = match validate_public_navigation_url(
                    &event.params.request.url,
                    state.network_policy,
                ) {
                    Ok(destination) => destination,
                    Err(error) => {
                        state.failure = Some(NavigationPolicyFailure::Rejected {
                            message: error.to_string(),
                        });
                        return RequestPausedDecision::Fail(FailRequest {
                            request_id,
                            error_reason: ErrorReason::BlockedByClient,
                        });
                    }
                };
                let approval_digest = navigation::redirect_approval_digest(
                    &state.original_url,
                    destination.canonical_url(),
                );
                let first_evaluation = state
                    .evaluated_destinations
                    .insert(destination.destination_digest().to_string());
                if first_evaluation {
                    if let Err(failure) = evaluate_navigation_destination(
                        &callback_policy,
                        &destination,
                        &approval_digest,
                        true,
                    ) {
                        state.failure = Some(failure);
                        return RequestPausedDecision::Fail(FailRequest {
                            request_id,
                            error_reason: ErrorReason::BlockedByClient,
                        });
                    }
                }
                state.request_chain.insert(request_id);
                RequestPausedDecision::Continue(None)
            },
        );
        let patterns = [RequestPattern {
            url_pattern: None,
            resource_Type: None,
            request_stage: Some(RequestStage::Request),
        }];
        let mut interception =
            NavigationInterceptionGuard::install(Arc::clone(&self.inner), interceptor, &patterns)?;

        let previous_url = self.current_url().ok();
        self.public_navigation_attempts
            .fetch_add(1, Ordering::Relaxed);
        let navigation_result =
            self.inner
                .navigate_to(requested.canonical_url())
                .context("public navigation request failed")
                .and_then(|_| {
                    self.inner.wait_until_navigated().map(|_| ()).or_else(|_| {
                        self.wait_for_public_navigation_fallback(previous_url.as_deref())
                    })
                });

        self.clear_stable_key_index();
        self.clear_semantic_capture_cache();
        // Conservatively advance after every started navigation. In particular,
        // this covers document commits followed by timeout/CDP failure. Delaying
        // the bump until interception completes lets a grant created for a blocked
        // redirect be consumed when the original URL is retried.
        self.navigation_epoch.fetch_add(1, Ordering::Relaxed);

        let interception_failure = interception_state
            .lock()
            .map_err(|_| anyhow::anyhow!("public navigation outcome is unavailable"))?
            .failure
            .clone();
        if let Ok(mut approvals) = self.policy_approvals.lock() {
            approvals.granted.clear();
            if !matches!(
                &interception_failure,
                Some(NavigationPolicyFailure::HumanApprovalRequired { .. })
            ) {
                approvals.pending = None;
            }
        }
        let cleanup_result = interception.finish();
        if let Some(failure) = interception_failure {
            return Err(failure.into_error());
        }
        navigation_result.context("public navigation did not complete")?;
        cleanup_result?;

        let final_url = self
            .current_url()
            .context("failed to read final public navigation URL")?;
        let final_url = validate_public_navigation_url_with(
            &final_url,
            NavigationNetworkPolicy::AllowPrivate,
            |_, _| Ok(Vec::new()),
        )?;
        Ok(final_url.canonical_url().to_string())
    }

    pub fn public_navigation_attempt_count(&self) -> u64 {
        self.public_navigation_attempts.load(Ordering::Relaxed)
    }

    pub(super) fn wait_for_public_navigation_fallback(
        &self,
        previous_url: Option<&str>,
    ) -> Result<()> {
        let started = Instant::now();
        loop {
            let current_url = self.current_url().ok();
            let ready_state = self.document_ready_state().ok();
            let dom_non_empty = self
                .get_content()
                .map(|content| !content.trim().is_empty())
                .unwrap_or(false);
            if current_url.as_deref() != previous_url
                && matches!(ready_state.as_deref(), Some("interactive" | "complete"))
                && dom_non_empty
            {
                return Ok(());
            }
            let remaining = remaining_timeout(started, NAVIGATION_FALLBACK_TIMEOUT);
            if remaining.is_zero() {
                anyhow::bail!("public navigation timed out");
            }
            thread::sleep(min(NAVIGATION_FALLBACK_POLL_INTERVAL, remaining));
        }
    }

    pub fn get_content(&self) -> Result<String> {
        self.inner
            .get_content()
            .context("Failed to get page content")
    }

    pub fn get_title(&self) -> Result<String> {
        self.inner.get_title().context("Failed to get page title")
    }

    pub fn current_url(&self) -> Result<String> {
        let value = self.evaluate_script_value("window.location.href", false)?;
        value
            .as_str()
            .map(ToOwned::to_owned)
            .context("Failed to resolve current page URL for policy evaluation")
    }

    pub(super) fn document_ready_state(&self) -> Result<String> {
        let value = self.evaluate_script_value("document.readyState", false)?;
        value
            .as_str()
            .map(ToOwned::to_owned)
            .context("Failed to resolve document.readyState while waiting for navigation")
    }

    pub(super) fn wait_for_navigation_fallback(
        &self,
        requested_url: &str,
        previous_url: Option<&str>,
    ) -> Result<()> {
        let started = Instant::now();
        let (last_url, last_ready_state, last_dom_non_empty) = loop {
            let current_url = self.current_url().ok();
            let ready_state = self.document_ready_state().ok();
            let dom_non_empty = self
                .get_content()
                .map(|content| !content.trim().is_empty())
                .unwrap_or(false);

            if navigation_fallback_condition_met(
                requested_url,
                previous_url,
                current_url.as_deref(),
                ready_state.as_deref(),
                dom_non_empty,
            ) {
                return Ok(());
            }

            let remaining = remaining_timeout(started, NAVIGATION_FALLBACK_TIMEOUT);
            if remaining.is_zero() {
                break (current_url, ready_state, dom_non_empty);
            }
            thread::sleep(min(NAVIGATION_FALLBACK_POLL_INTERVAL, remaining));
        };

        anyhow::bail!(
            "Navigation fallback timed out after {}ms (requested_url={}, previous_url={:?}, current_url={:?}, ready_state={:?}, dom_non_empty={})",
            duration_to_millis(NAVIGATION_FALLBACK_TIMEOUT),
            requested_url,
            previous_url,
            last_url,
            last_ready_state,
            last_dom_non_empty
        );
    }
}
