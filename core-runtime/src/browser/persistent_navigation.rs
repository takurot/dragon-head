use super::*;
use headless_chrome::protocol::cdp::{types::Event, Fetch, Network::ResourceType, Page};

#[derive(Clone)]
pub(super) struct DocumentSnapshot {
    pub(super) frame_id: String,
    pub(super) loader_id: String,
    // Private browser identity only: never emit the URL in an error or audit event.
    pub(super) url: String,
    pub(super) revision: u64,
}

pub(super) struct NavigationObservation {
    state: Mutex<ObservationState>,
    generation: AtomicU64,
}

struct ObservationState {
    document: DocumentSnapshot,
    child_frames: HashSet<String>,
    primary_navigation_complete: bool,
    awaiting_document_commit: bool,
    protected: bool,
    allow_private_network: bool,
    attempt: Option<NavigationAttempt>,
    automatic_origin: Option<String>,
    rejection: Option<(u64, NavigationPolicyFailure)>,
}

struct NavigationAttempt {
    original_url: String,
    explicit: bool,
    // Explicit retries keep the source reviewed before intermediate commits.
    approval_context_url: Option<String>,
    initial_seen: bool,
    requests: HashSet<String>,
    evaluated: HashSet<String>,
}

fn frame_url(frame: &Page::Frame) -> String {
    let mut url = frame.url.clone();
    if !url.contains('#') {
        if let Some(fragment) = &frame.url_fragment {
            url.push_str(fragment);
        }
    }
    url
}

fn without_fragment(url: &str) -> &str {
    url.split('#').next().unwrap_or(url)
}

fn child_frame_ids(tree: &Page::FrameTree, frames: &mut HashSet<String>) {
    for child in tree.child_frames.iter().flatten() {
        frames.insert(child.frame.id.clone());
        child_frame_ids(child, frames);
    }
}

impl NavigationObservation {
    pub(super) fn install(tab: &Arc<headless_chrome::Tab>) -> Result<Arc<Self>> {
        let tree = tab.call_method(Page::GetFrameTree(None))?.frame_tree;
        let frame = &tree.frame;
        let mut child_frames = HashSet::new();
        child_frame_ids(&tree, &mut child_frames);
        let observation = Arc::new(Self {
            state: Mutex::new(ObservationState {
                document: DocumentSnapshot {
                    frame_id: frame.id.clone(),
                    loader_id: frame.loader_id.clone(),
                    url: frame_url(frame),
                    revision: 0,
                },
                child_frames,
                primary_navigation_complete: false,
                awaiting_document_commit: false,
                protected: false,
                allow_private_network: false,
                attempt: None,
                automatic_origin: None,
                rejection: None,
            }),
            generation: AtomicU64::new(0),
        });
        let observer = Arc::clone(&observation);
        tab.add_event_listener(Arc::new(move |event: &Event| {
            let Ok(mut state) = observer.state.lock() else {
                return;
            };
            match event {
                Event::PageFrameNavigated(event) if event.params.frame.parent_id.is_none() => {
                    observer.record_frame(&mut state, &event.params.frame);
                }
                Event::PageLifecycleEvent(event) => {
                    Self::record_lifecycle(
                        &mut state,
                        &event.params.frame_id,
                        &event.params.loader_id,
                        &event.params.name,
                    );
                }
                Event::PageFrameAttached(event) => {
                    state.child_frames.insert(event.params.frame_id.clone());
                }
                Event::PageFrameNavigated(event) => {
                    state.child_frames.insert(event.params.frame.id.clone());
                }
                Event::PageFrameDetached(event) => {
                    state.child_frames.remove(&event.params.frame_id);
                }
                Event::PageNavigatedWithinDocument(event)
                    if event.params.frame_id == state.document.frame_id =>
                {
                    observer.record_url(&mut state, event.params.url.clone());
                }
                _ => {}
            }
        }))?;
        Ok(observation)
    }

    fn record_url(&self, state: &mut ObservationState, url: String) {
        if state.document.url != url {
            if without_fragment(&state.document.url) != without_fragment(&url) {
                state.document.revision += 1;
            }
            state.document.url = url;
            self.generation.fetch_add(1, Ordering::Release);
        }
    }

    fn record_lifecycle(state: &mut ObservationState, frame_id: &str, loader_id: &str, name: &str) {
        if frame_id == state.document.frame_id && loader_id == state.document.loader_id {
            match name {
                "init" => state.primary_navigation_complete = false,
                "networkAlmostIdle" => state.primary_navigation_complete = true,
                _ => {}
            }
        }
    }

    fn primary_navigation_complete(state: &ObservationState, previous: &DocumentSnapshot) -> bool {
        state.primary_navigation_complete
            && !state.awaiting_document_commit
            && (state.document.frame_id != previous.frame_id
                || state.document.loader_id != previous.loader_id)
    }

    fn record_frame(&self, state: &mut ObservationState, frame: &Page::Frame) {
        if state.document.frame_id != frame.id || state.document.loader_id != frame.loader_id {
            state.primary_navigation_complete = false;
            state.awaiting_document_commit = false;
            state.document.frame_id = frame.id.clone();
            state.document.loader_id = frame.loader_id.clone();
            state.document.revision += 1;
            self.generation.fetch_add(1, Ordering::Release);
        }
        self.record_url(state, frame_url(frame));
    }

    fn starting_request(&self, state: &mut ObservationState) {
        // Synchronous with the Fetch decision, before Chrome may send the request.
        // Scope binding cannot depend only on a later frameNavigated callback.
        state.primary_navigation_complete = false;
        state.awaiting_document_commit = true;
        state.document.revision += 1;
        self.generation.fetch_add(1, Ordering::Release);
    }
}

fn intercept_navigation_request(
    observation: &NavigationObservation,
    policy_context: &NavigationPolicyContext,
    event: &Fetch::events::RequestPausedEventParams,
) -> headless_chrome::browser::tab::RequestPausedDecision {
    use headless_chrome::browser::tab::RequestPausedDecision;
    let reject = || {
        RequestPausedDecision::Fail(Fetch::FailRequest {
            request_id: event.request_id.clone(),
            error_reason: headless_chrome::protocol::cdp::Network::ErrorReason::BlockedByClient,
        })
    };
    let Ok(mut state) = observation.state.lock() else {
        return reject();
    };
    if event.resource_Type != ResourceType::Document {
        return RequestPausedDecision::Continue(None);
    }
    if event.frame_id != state.document.frame_id {
        if state.child_frames.contains(&event.frame_id) {
            return RequestPausedDecision::Continue(None);
        }
        let revision = state.document.revision;
        state.rejection.get_or_insert((
            revision,
            NavigationPolicyFailure::Blocked {
                rule_id: "navigation-frame-identity".to_string(),
            },
        ));
        return reject();
    }
    observation.starting_request(&mut state);
    let policy = if state.allow_private_network {
        NavigationNetworkPolicy::AllowPrivate
    } else {
        NavigationNetworkPolicy::PublicOnly
    };
    let destination = match validate_public_navigation_url(&event.request.url, policy) {
        Ok(destination) => destination,
        Err(_) => {
            let revision = state.document.revision;
            state.rejection.get_or_insert((
                revision,
                NavigationPolicyFailure::Blocked {
                    rule_id: "navigation-network-policy".to_string(),
                },
            ));
            return reject();
        }
    };
    let continues_chain = event.redirected_request_id.as_ref().is_some_and(|parent| {
        state
            .attempt
            .as_ref()
            .is_some_and(|attempt| attempt.requests.contains(parent))
    });
    if !state
        .attempt
        .as_ref()
        .is_some_and(|attempt| attempt.explicit)
        && !continues_chain
    {
        let original_url = state
            .automatic_origin
            .clone()
            .unwrap_or_else(|| destination.canonical_url().to_string());
        state.attempt = Some(NavigationAttempt {
            original_url,
            explicit: false,
            approval_context_url: None,
            initial_seen: false,
            requests: HashSet::new(),
            evaluated: HashSet::new(),
        });
    }
    let source_url = state
        .attempt
        .as_ref()
        .and_then(|attempt| attempt.approval_context_url.as_ref())
        .unwrap_or(&state.document.url)
        .clone();
    let attempt = state.attempt.as_mut().expect("navigation attempt");
    let signature = if !attempt.initial_seen && destination.canonical_url() == attempt.original_url
    {
        destination.destination_digest().to_string()
    } else {
        navigation::redirect_approval_digest(&attempt.original_url, destination.canonical_url())
    };
    attempt.initial_seen = true;
    attempt.requests.insert(event.request_id.clone());
    let evaluate = !attempt.evaluated.contains(destination.destination_digest());
    drop(state);
    if evaluate {
        let context = NavigationPolicyContext {
            policy_engine: Arc::clone(&policy_context.policy_engine),
            policy_approvals: Arc::clone(&policy_context.policy_approvals),
            navigation_epoch: Arc::clone(&policy_context.navigation_epoch),
            audit_logger: Arc::clone(&policy_context.audit_logger),
            plugin_hooks: Arc::clone(&policy_context.plugin_hooks),
            approval_context_url: source_url,
        };
        if let Err(failure) =
            evaluate_navigation_destination(&context, &destination, &signature, true)
        {
            if let Ok(mut state) = observation.state.lock() {
                let revision = state.document.revision;
                state.rejection.get_or_insert((revision, failure));
            }
            return reject();
        }
        if let Ok(mut state) = observation.state.lock() {
            if let Some(attempt) = state.attempt.as_mut() {
                if attempt.requests.contains(&event.request_id) {
                    attempt
                        .evaluated
                        .insert(destination.destination_digest().to_string());
                }
            }
        }
    }
    RequestPausedDecision::Continue(None)
}

impl PageSession {
    pub fn navigation_revision(&self) -> u64 {
        self.navigation_observation
            .generation
            .load(Ordering::Acquire)
    }

    pub(super) fn live_document_snapshot(&self) -> Result<DocumentSnapshot> {
        for _ in 0..3 {
            let before = self.navigation_revision();
            let frame = self
                .inner
                .call_method(Page::GetFrameTree(None))?
                .frame_tree
                .frame;
            let mut state = self
                .navigation_observation
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("navigation observation is unavailable"))?;
            if before != self.navigation_revision() {
                continue;
            }
            self.navigation_observation.record_frame(&mut state, &frame);
            return Ok(state.document.clone());
        }
        anyhow::bail!("document changed while reading navigation identity")
    }

    pub(super) fn with_approval_document_context_current<T>(
        &self,
        expected: &DocumentSnapshot,
        operation: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        // Global order: observation -> approval. The operation must not call CDP.
        let state = self
            .navigation_observation
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("navigation observation is unavailable"))?;
        if state.document.revision != expected.revision
            || state.document.frame_id != expected.frame_id
            || state.document.loader_id != expected.loader_id
            || state.document.url != expected.url
        {
            anyhow::bail!("document changed before approval commit");
        }
        operation()
    }

    pub fn synchronize_navigation_observation(&self) -> Result<u64> {
        self.live_document_snapshot()?;
        Ok(self.navigation_revision())
    }

    pub fn check_navigation_rejection(&self) -> Result<()> {
        let rejection = self
            .navigation_observation
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("navigation observation is unavailable"))?
            .rejection
            .take();
        if let Some((_, failure)) = rejection {
            return Err(failure.into_error());
        }
        Ok(())
    }

    /// Protect all future main-frame HTTP(S) requests, not only explicit navigate calls.
    pub fn configure_public_navigation(&self, allow_private_network: bool) -> Result<()> {
        let mut owner = self
            .navigation_interception
            .lock()
            .map_err(|_| anyhow::anyhow!("navigation protection is unavailable"))?;
        {
            let mut state = self
                .navigation_observation
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("navigation observation is unavailable"))?;
            state.allow_private_network = allow_private_network;
            if owner.is_some() {
                return Ok(());
            }
            state.protected = true;
        }
        let observation = Arc::clone(&self.navigation_observation);
        let policy_context = NavigationPolicyContext {
            policy_engine: Arc::clone(&self.policy_engine),
            policy_approvals: Arc::clone(&self.policy_approvals),
            navigation_epoch: Arc::clone(&self.navigation_epoch),
            audit_logger: Arc::clone(&self.audit_logger),
            plugin_hooks: Arc::clone(&self.plugin_hooks),
            approval_context_url: String::new(),
        };
        let interceptor = Arc::new(
            move |_: Arc<headless_chrome::browser::transport::Transport>,
                  _: headless_chrome::browser::transport::SessionId,
                  event: Fetch::events::RequestPausedEvent| {
                intercept_navigation_request(&observation, &policy_context, &event.params)
            },
        );
        let patterns = [Fetch::RequestPattern {
            url_pattern: None,
            resource_Type: Some(ResourceType::Document),
            request_stage: Some(Fetch::RequestStage::Request),
        }];
        match NavigationInterceptionGuard::install(Arc::clone(&self.inner), interceptor, &patterns)
        {
            Ok(guard) => {
                *owner = Some(guard);
                Ok(())
            }
            Err(error) => {
                if let Ok(mut state) = self.navigation_observation.state.lock() {
                    state.protected = false;
                }
                Err(error)
            }
        }
    }

    pub(super) fn finish_action_navigation(&self) -> Result<()> {
        let protected = self
            .navigation_observation
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("navigation observation is unavailable"))?
            .protected;
        if !protected {
            return Ok(());
        }
        let deadline = Instant::now() + Duration::from_millis(80);
        loop {
            self.check_navigation_rejection()?;
            if Instant::now() >= deadline {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(2));
        }
    }

    pub(super) fn begin_action_navigation(&self) -> Result<()> {
        let mut state = self
            .navigation_observation
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("navigation observation is unavailable"))?;
        state.attempt = None;
        state.automatic_origin = None;
        Ok(())
    }

    pub(super) fn navigate_with_persistent_boundary(
        &self,
        url: &str,
        allow_private_network: bool,
    ) -> Result<String> {
        let _serialized = self
            .public_navigation_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("public navigation lock is unavailable"))?;
        self.check_navigation_rejection()?;
        let policy = if allow_private_network {
            NavigationNetworkPolicy::AllowPrivate
        } else {
            NavigationNetworkPolicy::PublicOnly
        };
        let requested = validate_public_navigation_url(url, policy)?;
        self.configure_public_navigation(allow_private_network)?;
        let snapshot = self.live_document_snapshot()?;
        let context = NavigationPolicyContext {
            policy_engine: Arc::clone(&self.policy_engine),
            policy_approvals: Arc::clone(&self.policy_approvals),
            navigation_epoch: Arc::clone(&self.navigation_epoch),
            audit_logger: Arc::clone(&self.audit_logger),
            plugin_hooks: Arc::clone(&self.plugin_hooks),
            approval_context_url: snapshot.url.clone(),
        };
        self.audit_logger.log(AuditEvent::ToolCall {
            tool_name: "navigate".to_string(),
            args: serde_json::json!({"destination": requested.sanitized_projection()}),
            timestamp: epoch_millis_u64(),
        });
        self.with_approval_document_context_current(&snapshot, || {
            evaluate_navigation_destination(
                &context,
                &requested,
                requested.destination_digest(),
                false,
            )
            .map_err(NavigationPolicyFailure::into_error)
        })?;
        {
            let mut state = self
                .navigation_observation
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("navigation observation is unavailable"))?;
            state.attempt = Some(NavigationAttempt {
                original_url: requested.canonical_url().to_string(),
                explicit: true,
                approval_context_url: Some(snapshot.url.clone()),
                initial_seen: false,
                requests: HashSet::new(),
                evaluated: HashSet::from([requested.destination_digest().to_string()]),
            });
            state.automatic_origin = Some(requested.canonical_url().to_string());
            self.navigation_observation.starting_request(&mut state);
        }
        self.public_navigation_attempts
            .fetch_add(1, Ordering::Relaxed);
        let result = self
            .inner
            .navigate_to(requested.canonical_url())
            .map(|_| ())
            .and_then(|_| {
                let primary_deadline = Instant::now() + NAVIGATION_PRIMARY_TIMEOUT;
                let fallback_deadline = primary_deadline + NAVIGATION_FALLBACK_TIMEOUT;
                loop {
                    self.check_navigation_rejection()?;
                    {
                        let state = self.navigation_observation.state.lock().map_err(|_| {
                            anyhow::anyhow!("navigation observation is unavailable")
                        })?;
                        if NavigationObservation::primary_navigation_complete(&state, &snapshot) {
                            return Ok(());
                        }
                    }
                    // Preserve Tab's lifecycle readiness wait. DOM readiness is
                    // the weaker fallback only after the primary wait expires.
                    if Instant::now() >= primary_deadline {
                        let current = self.live_document_snapshot()?;
                        if (current.loader_id != snapshot.loader_id || current.url != snapshot.url)
                            && matches!(
                                self.document_ready_state().ok().as_deref(),
                                Some("interactive" | "complete")
                            )
                        {
                            return Ok(());
                        }
                        if Instant::now() >= fallback_deadline {
                            anyhow::bail!("public navigation timed out");
                        }
                    }
                    thread::sleep(NAVIGATION_FALLBACK_POLL_INTERVAL);
                }
            });
        self.clear_stable_key_index();
        self.clear_semantic_capture_cache();
        self.navigation_epoch.fetch_add(1, Ordering::Relaxed);
        let interception_failure = {
            let mut state = self
                .navigation_observation
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("navigation observation is unavailable"))?;
            state.attempt = None;
            state.rejection.take().map(|(_, failure)| failure)
        };
        let requires_approval = matches!(
            &interception_failure,
            Some(NavigationPolicyFailure::HumanApprovalRequired { .. })
        ) || matches!(
            result
                .as_ref()
                .err()
                .and_then(|error| error.downcast_ref::<ActionError>()),
            Some(ActionError::HumanApprovalRequired { .. })
        );
        // Preserve explicit-navigation scope expiry. Intermediate document
        // observations must not clear a capability needed by the same retry.
        if let Ok(mut approvals) = self.policy_approvals.lock() {
            approvals.granted.clear();
            if !requires_approval {
                approvals.pending = None;
            }
        }
        if let Some(failure) = interception_failure {
            return Err(failure.into_error());
        }
        result.context("public navigation did not complete")?;
        let final_url = self.live_document_snapshot()?.url;
        Ok(validate_public_navigation_url_with(
            &final_url,
            NavigationNetworkPolicy::AllowPrivate,
            |_, _| Ok(Vec::new()),
        )?
        .canonical_url()
        .to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation() -> NavigationObservation {
        NavigationObservation {
            state: Mutex::new(ObservationState {
                document: DocumentSnapshot {
                    frame_id: "main".to_string(),
                    loader_id: "loader".to_string(),
                    url: "https://example.test/page#one".to_string(),
                    revision: 0,
                },
                child_frames: HashSet::new(),
                primary_navigation_complete: false,
                awaiting_document_commit: false,
                protected: true,
                allow_private_network: false,
                attempt: None,
                automatic_origin: None,
                rejection: None,
            }),
            generation: AtomicU64::new(0),
        }
    }

    #[test]
    fn navigation_observation_separates_fragment_cache_and_document_revisions() {
        let observation = observation();
        let mut state = observation.state.lock().unwrap();
        observation.record_url(&mut state, "https://example.test/page#two".to_string());
        assert_eq!(state.document.revision, 0);
        assert_eq!(observation.generation.load(Ordering::Acquire), 1);
        observation.record_url(
            &mut state,
            "https://example.test/page?changed#two".to_string(),
        );
        assert_eq!(state.document.revision, 1);
        assert_eq!(observation.generation.load(Ordering::Acquire), 2);
    }

    #[test]
    fn primary_readiness_requires_current_root_loader_and_a_new_document() {
        let observation = observation();
        let mut state = observation.state.lock().unwrap();
        let previous = state.document.clone();
        observation.record_url(&mut state, "https://example.test/other".to_string());
        NavigationObservation::record_lifecycle(&mut state, "main", "loader", "networkAlmostIdle");
        assert!(state.primary_navigation_complete);
        assert!(!NavigationObservation::primary_navigation_complete(
            &state, &previous
        ));
        observation.starting_request(&mut state);
        assert!(!state.primary_navigation_complete);
        NavigationObservation::record_lifecycle(&mut state, "main", "loader", "networkAlmostIdle");
        let committed_frame = serde_json::from_value(serde_json::json!({
            "id": "main", "loaderId": "new-loader", "url": "https://example.test/other",
            "secureContextType": "Secure", "crossOriginIsolatedContextType": "NotIsolated",
            "gatedAPIFeatures": []
        }))
        .unwrap();
        observation.record_frame(&mut state, &committed_frame);
        assert!(!state.primary_navigation_complete);
        for (frame, loader) in [("child", "new-loader"), ("main", "loader")] {
            NavigationObservation::record_lifecycle(&mut state, frame, loader, "networkAlmostIdle");
            assert!(!NavigationObservation::primary_navigation_complete(
                &state, &previous
            ));
        }
        NavigationObservation::record_lifecycle(
            &mut state,
            "main",
            "new-loader",
            "networkAlmostIdle",
        );
        assert!(NavigationObservation::primary_navigation_complete(
            &state, &previous
        ));
        NavigationObservation::record_lifecycle(&mut state, "main", "new-loader", "init");
        assert!(!state.primary_navigation_complete);
        // A -> committed B -> pending C -> late idle(B) must not
        // complete at B while the latest main-frame request has not committed.
        observation.starting_request(&mut state);
        NavigationObservation::record_lifecycle(
            &mut state,
            "main",
            "new-loader",
            "networkAlmostIdle",
        );
        assert!(!NavigationObservation::primary_navigation_complete(
            &state, &previous
        ));
        let mut final_frame = committed_frame;
        final_frame.loader_id = "final-loader".to_string();
        observation.record_frame(&mut state, &final_frame);
        NavigationObservation::record_lifecycle(
            &mut state,
            "main",
            "final-loader",
            "networkAlmostIdle",
        );
        assert!(NavigationObservation::primary_navigation_complete(
            &state, &previous
        ));
    }

    #[test]
    fn navigation_request_invalidates_document_context_before_any_commit() {
        let observation = observation();
        let mut state = observation.state.lock().unwrap();
        let before = state.document.clone();
        observation.starting_request(&mut state);
        assert_ne!(state.document.revision, before.revision);
        assert_eq!(state.document.loader_id, before.loader_id);
        assert_eq!(observation.generation.load(Ordering::Acquire), 1);
    }

    #[test]
    fn repeated_rejected_destination_never_becomes_an_allowed_cached_decision() {
        use headless_chrome::browser::tab::RequestPausedDecision;
        for action in [PolicyAction::Block, PolicyAction::RequireHumanApproval] {
            let observation = observation();
            {
                let mut state = observation.state.lock().unwrap();
                state.allow_private_network = true;
                state.attempt = Some(NavigationAttempt {
                    original_url: "http://127.0.0.1/start".to_string(),
                    explicit: true,
                    approval_context_url: Some(state.document.url.clone()),
                    initial_seen: true,
                    requests: HashSet::new(),
                    evaluated: HashSet::new(),
                });
            }
            let context = NavigationPolicyContext {
                policy_engine: Arc::new(Mutex::new(
                    PolicyEngine::try_new(vec![PolicyRule {
                        id: "reject-destination".to_string(),
                        domain: None,
                        path_prefix: Some("/denied".to_string()),
                        role: None,
                        text_regex: None,
                        context_regex: None,
                        action,
                        scope: (action == PolicyAction::RequireHumanApproval)
                            .then_some(ApprovalScope::ActionOnly),
                        outcome_projector: None,
                    }])
                    .unwrap(),
                )),
                policy_approvals: Arc::new(Mutex::new(PolicyApprovalState::default())),
                navigation_epoch: Arc::new(AtomicU64::new(0)),
                audit_logger: Arc::new(AuditLogger::new()),
                plugin_hooks: Arc::new(PluginHookConfig::default()),
                approval_context_url: String::new(),
            };
            for id in ["first", "second"] {
                let event: Fetch::events::RequestPausedEventParams = serde_json::from_value(serde_json::json!({
                    "requestId": id, "frameId": "main", "resourceType": "Document",
                    "request": {"url": "http://127.0.0.1/denied", "method": "GET", "headers": {},
                        "initialPriority": "High", "referrerPolicy": "no-referrer"}
                })).unwrap();
                assert!(matches!(
                    intercept_navigation_request(&observation, &context, &event),
                    RequestPausedDecision::Fail(_)
                ));
            }
            assert!(observation
                .state
                .lock()
                .unwrap()
                .attempt
                .as_ref()
                .unwrap()
                .evaluated
                .is_empty());
        }
    }

    #[test]
    fn explicit_intermediate_navigation_preserves_until_navigation_approval_context() {
        use headless_chrome::browser::tab::RequestPausedDecision;
        let source = "chrome-error://chromewebdata/";
        let original = "http://93.184.216.34/start";
        let destination = "http://93.184.216.34/approved";
        let observation = observation();
        {
            let mut state = observation.state.lock().unwrap();
            state.document.url = original.to_string();
            state.attempt = Some(NavigationAttempt {
                original_url: original.to_string(),
                explicit: true,
                approval_context_url: Some(source.to_string()),
                initial_seen: true,
                requests: HashSet::new(),
                evaluated: HashSet::new(),
            });
        }
        let context = NavigationPolicyContext {
            policy_engine: Arc::new(Mutex::new(
                PolicyEngine::try_new(vec![PolicyRule {
                    id: "approve-redirect".to_string(),
                    domain: None,
                    path_prefix: Some("/approved".to_string()),
                    role: None,
                    text_regex: None,
                    context_regex: None,
                    action: PolicyAction::RequireHumanApproval,
                    scope: Some(ApprovalScope::UntilNavigation),
                    outcome_projector: None,
                }])
                .unwrap(),
            )),
            policy_approvals: Arc::new(Mutex::new(PolicyApprovalState {
                pending: None,
                granted: vec![GrantedPolicyApproval {
                    request: PolicyApprovalRequest {
                        rule_id: "approve-redirect".to_string(),
                        scope: ApprovalScope::UntilNavigation,
                        action: "navigate".to_string(),
                        target_signature: navigation::redirect_approval_digest(
                            original,
                            destination,
                        ),
                        outcome: None,
                    },
                    granted_navigation_epoch: 0,
                    granted_url: source.to_string(),
                    expires_at_epoch_ms: None,
                    remaining_uses: None,
                }],
            })),
            navigation_epoch: Arc::new(AtomicU64::new(0)),
            audit_logger: Arc::new(AuditLogger::new()),
            plugin_hooks: Arc::new(PluginHookConfig::default()),
            approval_context_url: source.to_string(),
        };
        let event = serde_json::from_value(serde_json::json!({
            "requestId": "script-redirect", "frameId": "main", "resourceType": "Document",
            "request": {"url": destination, "method": "GET", "headers": {},
                "initialPriority": "High", "referrerPolicy": "no-referrer"}
        }))
        .unwrap();
        assert!(
            matches!(
                intercept_navigation_request(&observation, &context, &event),
                RequestPausedDecision::Continue(_)
            ),
            "intermediate document must not change the explicit retry approval context"
        );
        assert!(context.policy_approvals.lock().unwrap().pending.is_none());
    }

    #[test]
    fn confirmed_iframes_are_excluded_but_unknown_document_frames_fail_closed() {
        use headless_chrome::browser::tab::RequestPausedDecision;
        let observation = observation();
        observation
            .state
            .lock()
            .unwrap()
            .child_frames
            .insert("child".to_string());
        let context = NavigationPolicyContext {
            policy_engine: Arc::new(Mutex::new(PolicyEngine::default())),
            policy_approvals: Arc::new(Mutex::new(PolicyApprovalState::default())),
            navigation_epoch: Arc::new(AtomicU64::new(0)),
            audit_logger: Arc::new(AuditLogger::new()),
            plugin_hooks: Arc::new(PluginHookConfig::default()),
            approval_context_url: String::new(),
        };
        for (frame, url, excluded) in [
            ("child", "http://127.0.0.1/private", true),
            ("unknown", "http://93.184.216.34/", false),
        ] {
            let event: Fetch::events::RequestPausedEventParams = serde_json::from_value(serde_json::json!({
                "requestId": frame, "frameId": frame, "resourceType": "Document",
                "request": {"url": url, "method": "GET", "headers": {}, "initialPriority": "High", "referrerPolicy": "no-referrer"}
            })).unwrap();
            let decision = intercept_navigation_request(&observation, &context, &event);
            assert_eq!(
                matches!(decision, RequestPausedDecision::Continue(_)),
                excluded
            );
        }
    }

    #[test]
    fn pending_navigation_rejection_precedes_unrelated_action_side_effects() -> Result<()> {
        if test_bench_support::should_skip_browser_tests() {
            return Ok(());
        }
        let client = BrowserClient::new()?;
        let page = client.new_page()?;
        page.navigate(
            "data:text/html,%3Cbutton%20onclick%3D%27window.clicked%3D1%27%3Esafe%3C/button%3E",
        )?;
        page.configure_public_navigation(false)?;
        fn button_id(node: &SemanticNode) -> Option<i64> {
            if node.role == "button" {
                Some(node.backend_node_id)
            } else {
                node.children.iter().find_map(button_id)
            }
        }
        let state = page.capture_semantic_state(LoadProfile::Interactive)?;
        let id = button_id(state.root()).expect("button id");
        {
            let mut state = page.navigation_observation.state.lock().unwrap();
            state.rejection = Some((
                state.document.revision,
                NavigationPolicyFailure::Blocked {
                    rule_id: "late-navigation".to_string(),
                },
            ));
        }
        assert!(page.act(Some(id), None, "click", None).is_err());
        assert_eq!(
            page.evaluate_script_json("window.clicked || 0")?,
            serde_json::json!(0)
        );
        page.act(Some(id), None, "click", None)?;
        assert_eq!(
            page.evaluate_script_json("window.clicked")?,
            serde_json::json!(1)
        );
        Ok(())
    }
}
