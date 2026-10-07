use super::*;

impl PageSession {
    pub fn get_document_node(&self) -> Result<headless_chrome::protocol::cdp::DOM::Node> {
        // Enforce DOM domain enablement if not already?
        // Tab usually enables domains on demand or we might need to do it.
        // But let's try calling get_document.
        let root = self
            .inner
            .call_method(headless_chrome::protocol::cdp::DOM::GetDocument {
                depth: Some(1000), // Retrieve full depth? Or default? Default is usually deep?
                // spec says: "The maximum depth at which children should be retrieved, defaults to 1. Use -1 for the entire subtree".
                // We need full tree for SRE.
                // Using 1000 as a large enough depth since -1 (full) is not supported by headless_chrome u32 type.
                pierce: Some(false), // Keep the hot path on the main document tree unless explicitly needed.
            })?;
        Ok(root.root)
    }

    pub(super) fn ensure_sre_event_bridge(&self) -> Result<u64> {
        let script = format!(
            r#"
(() => {{
    const bridgeKey = Symbol.for("{SRE_EVENT_BRIDGE_SYMBOL}");
    const existing = window[bridgeKey];
    const isValidExisting =
        existing &&
        typeof existing === "object" &&
        Number.isFinite(existing.version) &&
        Array.isArray(existing.waiters) &&
        Array.isArray(existing.dirtyPaths) &&
        existing.dirtyPathSet &&
        typeof existing.dirtyPathSet.clear === "function";

    if (isValidExisting) {{
        return Math.floor(existing.version);
    }}

    if (existing && existing.observer && typeof existing.observer.disconnect === "function") {{
        try {{
            existing.observer.disconnect();
        }} catch (_ignored) {{}}
    }}

    const state = {{
        version: 0,
        waiters: [],
        dirtyPaths: [],
        dirtyPathSet: new Set(),
    }};

    const toSemanticPath = (node) => {{
        if (!node) {{
            return null;
        }}

        if (node.nodeType === Node.DOCUMENT_NODE) {{
            return 'root/#document';
        }}

        let current = node;
        if (current.nodeType !== Node.ELEMENT_NODE) {{
            current = current.parentElement;
        }}
        if (!current) {{
            return null;
        }}

        const parts = [];
        while (current && current.nodeType === Node.ELEMENT_NODE) {{
            parts.push(current.tagName.toLowerCase());
            current = current.parentElement;
        }}

        parts.reverse();
        parts.unshift('#document');
        return `root/${{parts.join("/")}}`;
    }};

    const queuePath = (path) => {{
        if (typeof path !== "string" || path.length === 0) {{
            return;
        }}
        if (state.dirtyPathSet.has(path)) {{
            return;
        }}

        if (state.dirtyPaths.length >= 512) {{
            state.dirtyPaths = ['root/#document'];
            state.dirtyPathSet.clear();
            state.dirtyPathSet.add('root/#document');
            return;
        }}

        state.dirtyPathSet.add(path);
        state.dirtyPaths.push(path);
    }};

    const queueNodeAndParent = (node) => {{
        if (!node) {{
            return;
        }}
        queuePath(toSemanticPath(node));
        queuePath(toSemanticPath(node.parentElement));
    }};

    const notify = (mutations = []) => {{
        for (const mutation of mutations) {{
            const target = mutation.target;
            const elementTarget =
                target && target.nodeType === Node.ELEMENT_NODE
                    ? target
                    : target && target.parentElement
                        ? target.parentElement
                        : null;
            queueNodeAndParent(elementTarget);

            if (mutation.type === "childList") {{
                for (const added of mutation.addedNodes || []) {{
                    queueNodeAndParent(added);
                }}
                for (const removed of mutation.removedNodes || []) {{
                    queueNodeAndParent(mutation.target);
                }}
            }}
        }}

        state.version += 1;
        const waiters = state.waiters.splice(0, state.waiters.length);
        for (const waiter of waiters) {{
            try {{
                waiter(state.version);
            }} catch (_ignored) {{}}
        }}
    }};

    const target = document.documentElement || document;
    const observer = new MutationObserver((mutations) => notify(mutations));
    observer.observe(target, {{
        subtree: true,
        childList: true,
        attributes: true,
        characterData: true,
    }});

    state.observer = observer;
    window[bridgeKey] = state;
    return state.version;
}})()
"#
        );

        let value = self
            .evaluate_script_value(&script, false)
            .context("Failed to initialize SRE event bridge")?;
        value_to_u64(&value).context("Invalid SRE event bridge version")
    }

    pub(super) fn wait_for_sre_event(
        &self,
        last_seen_version: u64,
        timeout: Duration,
    ) -> Result<u64> {
        let timeout_ms = duration_to_millis(timeout);
        let script = format!(
            r#"
(() => {{
    const bridgeKey = Symbol.for("{SRE_EVENT_BRIDGE_SYMBOL}");
    const state = window[bridgeKey];
    const isValidState =
        state &&
        typeof state === "object" &&
        Number.isFinite(state.version) &&
        Array.isArray(state.waiters);

    if (!isValidState) {{
        return Promise.resolve({last_seen_version});
    }}

    if (state.version > {last_seen_version}) {{
        return Promise.resolve(state.version);
    }}

    return new Promise((resolve) => {{
        let settled = false;
        const complete = (version) => {{
            if (settled) {{
                return;
            }}
            settled = true;
            clearTimeout(timer_id);
            const index = state.waiters.indexOf(waiter);
            if (index >= 0) {{
                state.waiters.splice(index, 1);
            }}
            resolve(version);
        }};

        const waiter = (version) => complete(version);
        const timer_id = setTimeout(() => complete(state.version), {timeout_ms});
        state.waiters.push(waiter);
    }});
}})()
"#
        );

        let value = self
            .evaluate_script_value(&script, true)
            .context("Failed while waiting for SRE event")?;
        value_to_u64(&value).context("Invalid SRE event version")
    }

    pub(super) fn take_sre_dirty_paths(&self) -> Result<Vec<String>> {
        let script = format!(
            r#"
(() => {{
    const bridgeKey = Symbol.for("{SRE_EVENT_BRIDGE_SYMBOL}");
    const state = window[bridgeKey];
    const isValidState =
        state &&
        typeof state === "object" &&
        Array.isArray(state.dirtyPaths) &&
        state.dirtyPathSet &&
        typeof state.dirtyPathSet.clear === "function";

    if (!isValidState) {{
        return [];
    }}

    const paths = state.dirtyPaths.slice();
    state.dirtyPaths = [];
    state.dirtyPathSet.clear();
    return paths;
}})()
"#
        );

        let value = self
            .evaluate_script_value(&script, false)
            .context("Failed to fetch dirty semantic paths")?;
        value_to_string_vec(&value)
    }

    /// Refresh the per-session stable_key index from the latest semantic capture.
    /// Returns the number of indexed nodes.
    pub fn refresh_stable_key_index(&self, profile: LoadProfile) -> Result<usize> {
        self.capture_state(profile)?;
        Ok(self
            .stable_key_index
            .lock()
            .map(|index| index.len())
            .unwrap_or_default())
    }

    /// Capture semantic state with the requested load profile.
    pub fn capture_semantic_state(&self, profile: LoadProfile) -> Result<SemanticState> {
        self.capture_state(profile)
    }

    /// Resolve a backend node id from the per-session stable_key index.
    /// The index is keyed by the first `STABLE_KEY_SHORT_LEN` hex chars; callers
    /// may pass either the full 64-char internal key or the 16-char external key.
    pub fn lookup_backend_node_id_by_stable_key(&self, stable_key: &str) -> Option<i64> {
        let short: String = stable_key.chars().take(STABLE_KEY_SHORT_LEN).collect();
        self.stable_key_index
            .lock()
            .ok()
            .and_then(|index| index.get(short.as_str()).map(|entry| entry.backend_node_id))
    }

    /// Resolve alias metadata from the per-session stable_key index.
    /// See [`lookup_backend_node_id_by_stable_key`] for key-length semantics.
    pub fn lookup_alias_by_stable_key(&self, stable_key: &str) -> Option<String> {
        let short: String = stable_key.chars().take(STABLE_KEY_SHORT_LEN).collect();
        self.stable_key_index.lock().ok().and_then(|index| {
            index
                .get(short.as_str())
                .and_then(|entry| entry.alias.clone())
        })
    }

    /// Wait until a semantic target reaches the requested state.
    pub fn wait_for_semantic(
        &self,
        target: SemanticTarget,
        desired_state: SemanticWaitState,
        timeout: Duration,
    ) -> Result<()> {
        self.wait_for_semantic_with_options(
            target,
            desired_state,
            timeout,
            SemanticWaitOptions::default(),
        )
    }

    /// Wait until a semantic target reaches the requested state with explicit options.
    pub fn wait_for_semantic_with_options(
        &self,
        target: SemanticTarget,
        desired_state: SemanticWaitState,
        timeout: Duration,
        options: SemanticWaitOptions,
    ) -> Result<()> {
        let mut subscriber = SreEventSubscriber::new(self, options.load_profile);
        let started = Instant::now();
        let transient_backoff = transient_error_backoff(options.poll_interval);

        loop {
            let remaining = remaining_timeout(started, timeout);
            if remaining.is_zero() {
                return Err(WaitError::Timeout {
                    operation: format!(
                        "semantic target {:?} to become {:?}",
                        target, desired_state
                    ),
                    timeout_ms: duration_to_millis(timeout),
                }
                .into());
            }

            match subscriber.wait_next_state_event(remaining) {
                Ok(Some(state)) => {
                    if target_matches_state(&state, &target, desired_state) {
                        return Ok(());
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    if !is_transient_capture_error(&err) {
                        return Err(err.context("Failed while waiting for semantic target state"));
                    }
                    sleep_transient_backoff(started, timeout, transient_backoff);
                }
            }
        }
    }

    /// Wait until the specified intent marker appears in semantic state updates.
    pub fn wait_for_intent(&self, intent: &str, timeout: Duration) -> Result<()> {
        self.wait_for_intent_with_options(intent, timeout, SemanticWaitOptions::default())
    }

    /// Wait until the specified intent marker appears with explicit options.
    pub fn wait_for_intent_with_options(
        &self,
        intent: &str,
        timeout: Duration,
        options: SemanticWaitOptions,
    ) -> Result<()> {
        let mut subscriber = SreEventSubscriber::new(self, options.load_profile);
        let started = Instant::now();
        let transient_backoff = transient_error_backoff(options.poll_interval);

        loop {
            let remaining = remaining_timeout(started, timeout);
            if remaining.is_zero() {
                return Err(WaitError::Timeout {
                    operation: format!("intent '{}'", intent),
                    timeout_ms: duration_to_millis(timeout),
                }
                .into());
            }

            match subscriber.wait_next_state_event(remaining) {
                Ok(Some(state)) => {
                    if state_contains_intent(state.root(), intent) {
                        return Ok(());
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    if !is_transient_capture_error(&err) {
                        return Err(err.context("Failed while waiting for intent"));
                    }
                    sleep_transient_backoff(started, timeout, transient_backoff);
                }
            }
        }
    }

    pub(super) fn capture_state(&self, profile: LoadProfile) -> Result<SemanticState> {
        let cache = self.semantic_capture_cache_snapshot(profile)?;
        let raw_state = Arc::new(self.capture_state_with_refinement(profile, &[], None, None)?);
        self.record_state_update(cache.last_state.clone(), Arc::clone(&raw_state))?;

        // Apply state plugin hooks for external delivery (non-fatal on failure).
        // NOTE: The raw (unmodified) state is stored in the semantic capture cache so
        // that policy enforcement in `enforce_policy()` always operates on the
        // original SRE output, not on plugin-transformed data.  This preserves the
        // trust boundary: plugins may only transform state for external consumers;
        // they cannot influence internal policy decisions.
        let transformed_state = self.apply_state_hooks(Arc::clone(&raw_state))?;

        self.replace_semantic_capture_cache(profile, Arc::clone(&raw_state))?;
        Ok(transformed_state.as_ref().clone())
    }

    /// Serialize the `SemanticState` root, run all state plugins, then
    /// deserialize the result.  On any failure (serialization, plugin error,
    /// deserialization), log to audit and return the original state unchanged.
    pub(super) fn apply_state_hooks(
        &self,
        state: Arc<SemanticState>,
    ) -> Result<Arc<SemanticState>> {
        let plugins = &self.plugin_hooks.state_plugins;
        if plugins.is_empty() {
            return Ok(state);
        }

        let state_json = match serde_json::to_string(state.root()) {
            Ok(json) => json,
            Err(err) => {
                tracing::warn!(error = %err, "failed to serialize state for plugin hooks");
                return Ok(state);
            }
        };

        let (transformed_json, events) =
            run_state_hooks(&state_json, plugins.iter().map(|p| p.as_ref()));

        for event in events {
            self.audit_logger.log(event);
        }

        if transformed_json == state_json {
            // Nothing changed — reuse the existing Arc.
            return Ok(state);
        }

        // Attempt to deserialize the transformed JSON back into a SemanticNode.
        match serde_json::from_str::<SemanticNode>(&transformed_json) {
            Ok(new_root) => {
                let new_state = Arc::new(SemanticState::new(new_root, state.load_profile()));
                Ok(new_state)
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "state plugin produced invalid JSON, reverting to original"
                );
                Ok(state)
            }
        }
    }

    /// Retrieve the actual browser viewport dimensions via JavaScript.
    ///
    /// Falls back to the default 800×600 constants if the CDP call fails, so that
    /// captures remain functional even when the browser context is unavailable.
    pub(super) fn get_viewport_dimensions(&self) -> ViewportDimensions {
        let script = "JSON.stringify({width: window.innerWidth, height: window.innerHeight})";
        let Ok(value) = self.evaluate_script_value(script, false) else {
            return ViewportDimensions::default();
        };
        let Some(json_str) = value.as_str() else {
            return ViewportDimensions::default();
        };
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str) else {
            return ViewportDimensions::default();
        };
        let width = parsed
            .get("width")
            .and_then(|v| v.as_f64())
            .filter(|&w| w > 0.0);
        let height = parsed
            .get("height")
            .and_then(|v| v.as_f64())
            .filter(|&h| h > 0.0);
        match (width, height) {
            (Some(w), Some(h)) => ViewportDimensions {
                width: w,
                height: h,
            },
            _ => ViewportDimensions::default(),
        }
    }

    pub(super) fn capture_state_with_refinement(
        &self,
        profile: LoadProfile,
        dirty_paths: &[String],
        cached_root: Option<&SemanticNode>,
        cached_paths: Option<&HashMap<String, Vec<usize>>>,
    ) -> Result<SemanticState> {
        let root = self.get_document_node()?;
        let viewport = self.get_viewport_dimensions();
        let normalized_dirty_paths = normalize_dirty_paths(dirty_paths);
        let sem_root = if let (Some(cached_root), Some(cached_paths)) = (cached_root, cached_paths)
        {
            if !normalized_dirty_paths.is_empty() && !cached_paths.is_empty() {
                normalize_dom_with_viewport_and_refinement(
                    profile,
                    &root,
                    viewport,
                    SubtreeRefinementConfig {
                        dirty_paths: &normalized_dirty_paths,
                        cached_paths,
                        cached_root,
                    },
                )?
            } else {
                normalize_dom_with_viewport(profile, &root, viewport)?
            }
        } else {
            normalize_dom_with_viewport(profile, &root, viewport)?
        };
        self.replace_stable_key_index(&sem_root);
        Ok(SemanticState::new(sem_root, profile))
    }

    pub(super) fn replace_stable_key_index(&self, root: &SemanticNode) {
        if let Ok(mut index) = self.stable_key_index.lock() {
            index.clear();
            collect_stable_key_entries(root, &mut index);
        }
    }

    pub(super) fn clear_stable_key_index(&self) {
        if let Ok(mut index) = self.stable_key_index.lock() {
            index.clear();
        }
    }

    pub(super) fn semantic_capture_cache_snapshot(
        &self,
        profile: LoadProfile,
    ) -> Result<SemanticCaptureCache> {
        let cache = self
            .semantic_capture_cache
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock semantic capture cache"))?
            .clone();

        if cache.profile == Some(profile) {
            Ok(cache)
        } else {
            Ok(SemanticCaptureCache::default())
        }
    }

    pub(super) fn replace_semantic_capture_cache(
        &self,
        profile: LoadProfile,
        state: Arc<SemanticState>,
    ) -> Result<()> {
        let mut cache = self
            .semantic_capture_cache
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock semantic capture cache"))?;
        cache.profile = Some(profile);
        cache.last_state = Some(state);
        Ok(())
    }

    pub(super) fn clear_semantic_capture_cache(&self) {
        if let Ok(mut cache) = self.semantic_capture_cache.lock() {
            *cache = SemanticCaptureCache::default();
        }
    }

    pub(super) fn record_state_update(
        &self,
        previous: Option<Arc<SemanticState>>,
        current: Arc<SemanticState>,
    ) -> Result<()> {
        self.audit_logger.log_state_update(previous, current);
        Ok(())
    }
}
