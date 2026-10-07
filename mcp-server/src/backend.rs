use super::*;

impl CoreRuntimeBackend {
    pub fn new(page: PageSession) -> Self {
        Self {
            page: Arc::new(page),
            state_cache: None,
            previous_semantic_state: None,
            skill_engine: SkillEngine::new(),
            skills: HashMap::new(),
            last_skill_delta: SkillUsageDelta::default(),
            pending_visual_image: None,
            schema_registry: SchemaRegistry::with_builtin_rules(),
            injection_sanitizer: PromptInjectionSanitizer::new(PromptInjectionSanitizerConfig {
                mode: PromptInjectionMode::ReportOnly,
                ..Default::default()
            }),
            client: None,
            policy_rules: Vec::new(),
            navigation_allow_private_network: false,
            browser_restarts: 0,
            restart_history: VecDeque::new(),
            speculative: SpeculativeEngine::new(Vec::new()),
            last_action: None,
            pending_action: None,
            speculative_hits: 0,
            speculative_misses: 0,
            last_served_prediction: None,
            action_chain_broken: false,
            previous_state_verified: true,
            last_served_prediction_source_hash: None,
        }
    }

    /// Like [`new`](Self::new), but retains the managed [`BrowserClient`] so that
    /// the session can be automatically relaunched after a Chrome crash or
    /// disconnect (ISSUE-149).
    pub fn new_with_client(client: BrowserClient, page: PageSession) -> Self {
        // Capture any policy rules already configured on `page` so they
        // survive a future browser restart (ISSUE-149 review feedback).
        let policy_rules = page.policy_rules().unwrap_or_default();
        Self {
            client: Some(client),
            policy_rules,
            ..Self::new(page)
        }
    }

    /// Replaces the prompt-injection sanitizer with one configured for `mode`. Used at startup
    /// to apply the resolved `prompt_injection.mode` from `config::resolve_config`.
    pub fn set_injection_mode(&mut self, mode: PromptInjectionMode) {
        self.set_injection_config(PromptInjectionSanitizerConfig {
            mode,
            ..Default::default()
        });
    }

    /// Replaces the prompt-injection sanitizer with the resolved startup config.
    pub fn set_injection_config(&mut self, config: PromptInjectionSanitizerConfig) {
        self.injection_sanitizer = PromptInjectionSanitizer::new(config);
    }

    /// Applies `rules` to the current page session and stores them so they can be
    /// reapplied to the page created by a future browser restart (ISSUE-149).
    pub fn set_policy_rules(&mut self, rules: Vec<PolicyRule>) -> Result<()> {
        self.page.set_policy_rules(rules.clone())?;
        self.policy_rules = rules;
        Ok(())
    }

    pub fn set_navigation_allow_private_network(&mut self, allow: bool) {
        self.navigation_allow_private_network = allow;
    }

    /// Invalidates every page-local MCP and speculative cursor after browser I/O begins.
    /// Learned speculative transitions, cumulative hit/miss counters, skills, and policy rules
    /// intentionally survive navigation.
    pub(crate) fn reset_navigation_state(&mut self) {
        self.state_cache = None;
        self.previous_semantic_state = None;
        self.pending_action = None;
        self.last_action = None;
        self.last_served_prediction = None;
        self.last_served_prediction_source_hash = None;
        self.action_chain_broken = false;
        self.previous_state_verified = true;
        self.speculative.clear_action_cursor();
    }

    /// Treats a `run_skill` execution that performed at least one successful `act` step as an
    /// opaque mutation boundary (ISSUE-301): `PageSkillRuntime::act` mutates the live page
    /// directly, outside of `CoreRuntimeBackend::act`'s bookkeeping, so `state_cache`,
    /// `previous_semantic_state`, and the speculative action/prediction cursors can no longer be
    /// trusted to describe the current page. A multi-action skill also has no single
    /// `ActionSignature` describing its combined effect, so the same conservative invalidation
    /// applied on navigation is applied here rather than attempting to track it. This must run
    /// even when the skill run ultimately fails, as long as at least one `act` step already
    /// succeeded, so a partial-success skill cannot leave stale state behind.
    pub(crate) fn invalidate_after_opaque_skill_mutation(&mut self) {
        self.reset_navigation_state();
    }

    /// Best-effort explanation of why `rule` produced a null/empty/partial result. A failure of
    /// the diagnostics script itself must never mask the extraction result, so it yields no
    /// entries.
    pub(crate) fn diagnose_extraction(
        &self,
        rule: &ExtractionRule,
    ) -> serde_json::Map<String, Value> {
        match self
            .page
            .evaluate_script_json(&rule.to_js_diagnostics_script())
        {
            Ok(Value::Object(map)) => map,
            Ok(_) | Err(_) => serde_json::Map::new(),
        }
    }

    pub fn register_extraction_rule(&mut self, name: &str, value: &Value) -> Result<()> {
        self.schema_registry
            .register(name, value)
            .with_context(|| format!("failed to register extraction rule '{name}'"))
    }

    pub fn page(&self) -> &PageSession {
        &self.page
    }

    /// Returns a cloned handle to the exact `PageSession` this backend acts on.
    ///
    /// `PageSession`'s policy-approval methods (`pending_policy_approval`,
    /// `approve_pending_policy_action`, `reject_pending_policy_action`) take `&self` and are
    /// safe to call from another thread, so this lets an in-process HITL bridge
    /// (ISSUE-302) observe and resolve the same pending approvals as this backend's own
    /// `ask_human`, instead of independently opening an unrelated `PageSession`.
    ///
    /// Note: [`CoreRuntimeBackend::handle_browser_disconnect`] replaces `self.page` with a
    /// freshly relaunched session after a Chrome crash (ISSUE-149); a handle obtained before
    /// that point keeps referring to the old, now-defunct session rather than following the
    /// restart.
    pub fn page_handle(&self) -> Arc<PageSession> {
        Arc::clone(&self.page)
    }

    pub fn register_skill(&mut self, skill: SkillDefinition) {
        self.skills.insert(skill.name.clone(), skill);
    }

    pub fn register_skill_json(&mut self, value: &Value) -> Result<()> {
        let skill = parse_skill_definition(value)?;
        self.register_skill(skill);
        Ok(())
    }

    pub(crate) fn semantic_state_payload(
        &mut self,
        force_refresh: bool,
    ) -> Result<ExternalSemanticState> {
        if !force_refresh {
            if let Some(cached) = &self.state_cache {
                return Ok(cached.clone());
            }
        }

        let (snapshot, prediction, outcome) = resolve_speculative_state(
            &self.speculative,
            self.previous_semantic_state.as_ref(),
            self.last_action.as_ref(),
            self.pending_action.as_ref(),
            force_refresh,
            self.last_served_prediction.is_some(),
        );

        if let (SpeculativeOutcome::Hit, Some(snapshot)) = (&outcome, snapshot) {
            let state = (*snapshot).clone();
            let payload = self.build_external_state(state.clone(), true)?;
            self.speculative_hits += 1;
            // Replace any existing unverified prediction with this hit's
            // (Spec §3.5 / ISSUE-147 review): if a previous hit's prediction
            // is still pending here, this hit's action has moved the page
            // beyond the state that prediction described, so verifying it
            // against a future real capture would report a false mismatch.
            // The new prediction describes `state` (the snapshot just
            // served), which the next real capture *can* validate against
            // (assuming no further chaining), so it replaces the stale one.
            // Capture the `from_state_hash` this prediction was generated
            // from *before* it is overwritten below (Spec §3.5 /
            // ISSUE-147 round-10 review): if the prediction turns out to be
            // stale, `record_speculative_observation` needs this hash to
            // correct the `(from_state_hash, predicted_action)` entry in
            // the transition model.
            self.last_served_prediction_source_hash = self
                .previous_semantic_state
                .as_ref()
                .map(|s| s.state_hash().to_string());
            self.last_served_prediction = prediction;
            self.last_action = self.pending_action.take();
            self.previous_semantic_state = Some(state);
            // `state` is an unverified speculative snapshot (Spec §3.5 /
            // ISSUE-147 round-9 review): until a real capture reconciles it,
            // `record_speculative_observation` must not train a transition
            // from it.
            self.previous_state_verified = false;
            // Deliberately do NOT populate `state_cache` (Spec §3.5 /
            // ISSUE-147 review): caching this unverified speculative payload
            // would make every subsequent non-forced `get_state` return it
            // via the early-return above, so `record_speculative_observation`
            // would never run and `last_served_prediction` would never be
            // checked against a real capture. Leaving the cache empty means
            // the next `get_state` falls through to a real capture (since
            // `pending_action` was just consumed above), reconciling this
            // prediction against the live DOM.
            return Ok(payload);
        }

        let raw_state = self.page.capture_semantic_state(LoadProfile::Interactive)?;
        let state = raw_state.sanitized_with(&self.injection_sanitizer);

        let was_chain_broken = self.action_chain_broken;
        self.record_speculative_observation(&state);

        if matches!(outcome, SpeculativeOutcome::MissPreGenerateNone) {
            if let Some(prediction) = &prediction {
                self.speculative.verify(prediction, &state);
            }
        }

        if matches!(
            outcome,
            SpeculativeOutcome::MissNoPrediction
                | SpeculativeOutcome::MissPredictionMismatch
                | SpeculativeOutcome::MissPreGenerateNone
        ) {
            self.speculative_misses += 1;
        }

        let state_copy = state.clone();
        let payload = self.build_external_state(state, false)?;

        // Only promote `pending_action` to `last_action` when an action
        // actually occurred (Spec §3.5 / ISSUE-147 round-11 review):
        // leaving `last_action` unchanged on duplicate captures (no
        // intervening `act`) preserves the learned action-sequence history
        // so future speculation can still use it. Chain breaks (where
        // `act()` already cleared both cursors) are identified via
        // `was_chain_broken` captured before `record_speculative_observation`
        // reset `action_chain_broken`.
        if let Some(action) = self.pending_action.take() {
            self.last_action = Some(action);
        } else if was_chain_broken {
            self.last_action = None;
        }
        self.previous_semantic_state = Some(state_copy);
        self.state_cache = Some(payload.clone());
        Ok(payload)
    }

    /// Records bookkeeping for the speculative engine after a real DOM
    /// capture (Spec §3.5 / ISSUE-147):
    ///
    /// - Verifies the prediction behind the most recently served speculative
    ///   hit (if any) against this freshly-captured `state`, logging a
    ///   mismatch for replay/debugging when the served snapshot turns out to
    ///   have been stale. This is only valid when no action has been
    ///   executed since the hit was served (`pending_action.is_none()`) and
    ///   no chained-action sequence has been discarded
    ///   (`!action_chain_broken`): in either case `state` reflects the page
    ///   after more than the predicted single action, not the state the
    ///   prediction described, and comparing the two would report a false
    ///   mismatch (Spec §3.5 / ISSUE-147 review). In that case the stale
    ///   prediction is discarded without verification. When verification
    ///   *does* report a mismatch, the stale
    ///   `(last_served_prediction_source_hash, served.predicted_action) ->
    ///   predicted_state_hash` transition is corrected to point at
    ///   `actual_state_hash` instead, so the same known-wrong snapshot is
    ///   not served again on a repeat of this action (Spec §3.5 /
    ///   ISSUE-147 round-10 review).
    /// - Caches `state` so it can be served by a future pre-generation.
    /// - Records the `previous_semantic_state -> state` transition under
    ///   `pending_action`, if one was executed since the last capture and
    ///   `previous_semantic_state` is itself a verified (real) capture
    ///   (`previous_state_verified`). If `previous_semantic_state` is an
    ///   unverified speculative snapshot, training a transition from it
    ///   risks recording an edge that does not exist in the live page's
    ///   transition graph (Spec §3.5 / ISSUE-147 round-9 review). Either
    ///   way, if `pending_action` is set, the engine's action-sequence
    ///   cursor is advanced to match so the next `record_transition` links
    ///   to the correct prior action (Spec §3.5 / ISSUE-147 round-10
    ///   review). If `pending_action` is `None` (duplicate capture, no
    ///   intervening `act`), both cursors are left unchanged to preserve
    ///   the learned action-sequence history (round-11 review).
    pub(crate) fn record_speculative_observation(&mut self, state: &SemanticState) {
        reconcile_served_prediction(
            &self.speculative,
            &mut self.last_served_prediction,
            &mut self.last_served_prediction_source_hash,
            self.pending_action.as_ref(),
            self.action_chain_broken,
            state,
        );

        self.speculative.observe_state(Arc::new(state.clone()));

        if should_record_transition(
            self.previous_state_verified,
            self.previous_semantic_state.as_ref(),
            self.pending_action.as_ref(),
        ) {
            let previous = self
                .previous_semantic_state
                .as_ref()
                .expect("checked by should_record_transition");
            let pending = self
                .pending_action
                .as_ref()
                .expect("checked by should_record_transition");
            self.speculative
                .record_transition(previous.state_hash(), pending, state.state_hash());
        } else if let Some(pending) = self.pending_action.as_ref() {
            // `record_transition` was skipped, but `pending` was still
            // executed and is about to be promoted to `self.last_action`
            // below (Spec §3.5 / ISSUE-147 round-10 review). Advance the
            // engine's internal action-sequence cursor to match, so the
            // next `record_transition` links its action to `pending` rather
            // than to whichever action the cursor was last left on.
            self.speculative.advance_action_cursor(pending);
        }
        // When `pending_action` is `None` and no chain was broken: a real
        // capture happened with no intervening action (e.g. two consecutive
        // `get_state` calls). Leave the engine cursor and `self.last_action`
        // unchanged so the learned action-sequence history is preserved
        // (Spec §3.5 / ISSUE-147 round-11 review). For the chain-break
        // case (`action_chain_broken == true`), `clear_action_cursor()` was
        // already called in `act()` before this capture, so no reset needed
        // here either.

        self.action_chain_broken = false;
        // `state` is this real DOM capture, which the caller is about to
        // assign to `previous_semantic_state` (Spec §3.5 / ISSUE-147
        // round-9 review): future transitions may be trained from it.
        self.previous_state_verified = true;
    }

    pub(crate) fn build_external_state(
        &mut self,
        state: SemanticState,
        speculative: bool,
    ) -> Result<ExternalSemanticState> {
        let metadata = StateMetadata {
            url: self.page.current_url()?,
            page_instance_id: state.page_instance_id().to_string(),
            state_hash: state.state_hash().to_string(),
            load_profile: load_profile_name(state.load_profile()).to_string(),
            timestamp: state.timestamp(),
            speculative,
        };

        let interactive_elements = state
            .generate_fast_state()
            .interactive_elements
            .into_iter()
            .map(|node| self.map_interactive_element(node, speculative))
            .collect::<Result<Vec<_>>>()?;

        Ok(ExternalSemanticState {
            metadata,
            interactive_elements,
        })
    }

    /// Maps a `SemanticNode` to its external representation.
    ///
    /// For a speculative snapshot (Spec §3.5 / ISSUE-147), `backend_node_id`
    /// values were captured from a prior DOM render and may not resolve on
    /// the live page until the predicted transition actually occurs. The id
    /// and `stable_key` are still reported so `act`/`verify` can target the
    /// element (`act` and `verify_text` both fall back to `stable_key` if the
    /// id no longer resolves); only the `get_element_bbox` CDP lookup, which
    /// would fail or return stale coordinates for a not-yet-rendered node, is
    /// skipped.
    pub(crate) fn map_interactive_element(
        &self,
        node: SemanticNode,
        speculative: bool,
    ) -> Result<ExternalInteractiveElement> {
        let id = node.backend_node_id;
        let stable_key = shorten_key(
            &node
                .stable_key
                .clone()
                .filter(|key| !key.trim().is_empty())
                .unwrap_or_else(|| fallback_stable_key(&node)),
        );
        let alias = node
            .alias
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| fallback_alias(&node, &stable_key));
        let name = node
            .label
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| alias.clone());

        let bbox = if !speculative && id > 0 {
            self.page
                .get_element_bbox(id)?
                .unwrap_or([0.0, 0.0, 0.0, 0.0])
        } else {
            [0.0, 0.0, 0.0, 0.0]
        };

        let policy_flags = infer_policy_flags(&node);
        let security_flags = node.security_flags.clone();
        let attributes = node
            .attributes
            .unwrap_or_default()
            .into_iter()
            .map(|(key, value)| (key, parse_attribute_value(&value)))
            .collect();

        Ok(ExternalInteractiveElement {
            id,
            stable_key,
            alias,
            role: node.role,
            name,
            attributes,
            bbox,
            policy_flags,
            security_flags,
        })
    }
}
