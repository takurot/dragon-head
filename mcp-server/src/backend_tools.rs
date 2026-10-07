use super::*;

impl McpBackend for CoreRuntimeBackend {
    fn navigate(&mut self, arguments: Value) -> Result<Value> {
        let args: NavigateArguments =
            serde_json::from_value(arguments).context("invalid navigate arguments")?;
        // Produce the fragment-free public response identity without performing a duplicate DNS
        // lookup. `navigate_public` applies the configured network policy before browser I/O and
        // repeats it for redirects.
        let requested =
            validate_public_navigation_url(&args.url, NavigationNetworkPolicy::AllowPrivate)?;
        let requested_url = requested.canonical_url().to_string();
        let attempts_before = self.page.public_navigation_attempt_count();

        let result = self
            .page
            .navigate_public(&requested_url, self.navigation_allow_private_network);
        let browser_io_started = self.page.public_navigation_attempt_count() > attempts_before;
        if browser_io_started {
            self.reset_navigation_state();
        }

        match result {
            Ok(final_url) => {
                if !browser_io_started {
                    self.reset_navigation_state();
                }
                Ok(json!({
                    "status": "ok",
                    "requested_url": requested_url,
                    "final_url": final_url
                }))
            }
            Err(err) => {
                if let Some(action_err) = err.downcast_ref::<ActionError>() {
                    return Ok(action_error_payload(action_err));
                }
                Err(err.context("navigate tool failed"))
            }
        }
    }

    fn get_state(&mut self, arguments: Value) -> Result<Value> {
        let args = parse_get_state_arguments(&arguments)?;

        match args.delivery {
            StateDelivery::Full => {
                let payload = self.semantic_state_payload(args.force_refresh)?;
                match args.format {
                    StateFormat::Json => Ok(serde_json::to_value(payload)?),
                    StateFormat::Markdown => Ok(json!({
                        "markdown": render_state_markdown(&payload)
                    })),
                }
            }
            StateDelivery::Delta => {
                if args.force_refresh {
                    // Reset both caches so the next delta starts from a clean baseline.
                    self.state_cache = None;
                    self.previous_semantic_state = None;
                    self.previous_state_verified = true;
                    // Also clear the speculative action/prediction history
                    // (Spec §3.5 / ISSUE-147 review): with
                    // `previous_semantic_state` cleared,
                    // `record_speculative_observation` below cannot record a
                    // `previous -> current` transition for `pending_action`,
                    // but it would still be promoted to `last_action`
                    // afterwards. That would desync the backend's
                    // `last_action` from the `SpeculativeEngine`'s internal
                    // action history (only updated by `record_transition`),
                    // causing the next prediction to train/query the wrong
                    // action sequence. `last_served_prediction` similarly
                    // referred to the now-discarded baseline state. Discard
                    // all of it along with the state baseline.
                    self.pending_action = None;
                    self.last_action = None;
                    self.last_served_prediction = None;
                    self.last_served_prediction_source_hash = None;
                    self.action_chain_broken = false;
                    // Likewise reset the engine's own action-sequence cursor
                    // (Spec §3.5 / ISSUE-147 review): otherwise the next
                    // `record_transition` would link its action to whatever
                    // action preceded this forced baseline reset, training a
                    // false action-sequence edge.
                    self.speculative.clear_action_cursor();
                }

                let current = self.page.capture_semantic_state(LoadProfile::Interactive)?;
                let current = current.sanitized_with(&self.injection_sanitizer);
                let update = current.select_update(
                    self.previous_semantic_state.as_ref(),
                    DeltaPolicy::default(),
                )?;

                let response = match &update {
                    StateUpdate::Noop { state_hash } => {
                        json!({ "type": "no_change", "hash": state_hash })
                    }
                    StateUpdate::Full { state } => {
                        let ext = self.build_external_state(state.clone(), false)?;
                        // Keep state_cache in sync so a subsequent Full call is consistent.
                        self.state_cache = Some(ext.clone());
                        json!({
                            "type": "full",
                            "hash": state.state_hash().to_string(),
                            "state": serde_json::to_value(ext)?
                        })
                    }
                    StateUpdate::Delta { delta } => {
                        json!({
                            "type": "delta",
                            "base_hash": delta.previous_state_hash,
                            "next_hash": delta.next_state_hash,
                            "patch": serde_json::to_value(&delta.patch)?
                        })
                    }
                };

                // Reconcile speculative bookkeeping against this real capture
                // (Spec §3.5 / ISSUE-147 review): without this, `pending_action`
                // and `last_action` would go stale across a delta read, causing
                // a later full `get_state` to treat an already-applied action as
                // newly executed and serve a snapshot for a state past the
                // current one.
                let was_chain_broken = self.action_chain_broken;
                self.record_speculative_observation(&current);
                if let Some(action) = self.pending_action.take() {
                    self.last_action = Some(action);
                } else if was_chain_broken {
                    self.last_action = None;
                }
                self.previous_semantic_state = Some(current);
                Ok(response)
            }
        }
    }

    fn act(&mut self, arguments: Value) -> Result<Value> {
        let args: ActArguments =
            serde_json::from_value(arguments).context("invalid act arguments")?;

        // When a speculative hit was served but not yet verified against the
        // real DOM (no `get_state` real-capture since the hit), verify it
        // *before* this action alters the page (Spec §3.5 / ISSUE-147
        // round-11 review). Verification is only valid when no action has
        // been chained since the hit (`pending_action.is_none() &&
        // !action_chain_broken`). On mismatch, the stale transition is
        // corrected immediately so the same wrong snapshot isn't served again.
        // On capture failure, the verification block is skipped and
        // `last_served_prediction` is discarded later by
        // `record_speculative_observation` as in the pre-round-11 path.
        if self.last_served_prediction.is_some()
            && self.pending_action.is_none()
            && !self.action_chain_broken
        {
            if let Ok(real_pre) = self.page.capture_semantic_state(LoadProfile::Interactive) {
                let real_pre = real_pre.sanitized_with(&self.injection_sanitizer);
                reconcile_served_prediction(
                    &self.speculative,
                    &mut self.last_served_prediction,
                    &mut self.last_served_prediction_source_hash,
                    self.pending_action.as_ref(),
                    self.action_chain_broken,
                    &real_pre,
                );
                // Cache the verified real pre-action state. The engine cursor
                // is updated by `correct_transition` above (or left at the
                // previous action's cursor if the prediction was correct),
                // so `self.last_action` and `inner.last_action` both still
                // point at the action that led to `real_pre` — which is
                // correct for training the upcoming `record_transition`.
                self.speculative.observe_state(Arc::new(real_pre.clone()));
                self.previous_semantic_state = Some(real_pre);
                self.previous_state_verified = true;
            }
        }

        match self.page.act(
            args.target_id,
            args.target_stable_key.as_deref(),
            &args.action,
            args.value.as_deref(),
        ) {
            Ok(()) => {
                self.state_cache = None;
                if self.pending_action.is_some() || self.action_chain_broken {
                    // A second (or later) successful `act` before the next
                    // `get_state` (Spec §3.5 / ISSUE-147 review): no single
                    // `ActionSignature` describes the combined effect of
                    // multiple actions on `previous_semantic_state`, so
                    // recording a `previous -> current` transition under
                    // just this action would train the engine on an
                    // impossible transition that could later serve a
                    // snapshot for the wrong state. Discard the pending
                    // action and keep the chain flagged (it stays flagged
                    // until the next real capture resets it) so
                    // `record_speculative_observation` skips both training
                    // and `last_served_prediction` verification for this
                    // capture, no matter how many actions were chained.
                    // Also reset the engine's own action-sequence cursor
                    // (Spec §3.5 / ISSUE-147 review): otherwise the next
                    // `record_transition` would link its action to whatever
                    // action preceded this discarded chain, training a
                    // false action-sequence edge.
                    self.pending_action = None;
                    self.action_chain_broken = true;
                    self.speculative.clear_action_cursor();
                } else {
                    self.pending_action = Some(action_signature_for_act(&args));
                }
                Ok(json!({"status": "ok"}))
            }
            Err(err) => {
                if let Some(action_err) = err.downcast_ref::<ActionError>() {
                    return Ok(action_error_payload(action_err));
                }
                Err(err.context("act tool failed"))
            }
        }
    }

    fn verify(&mut self, arguments: Value) -> Result<Value> {
        let args: VerifyArguments =
            serde_json::from_value(arguments).context("invalid verify arguments")?;

        match self.page.verify_text(
            args.target_id,
            args.target_stable_key.as_deref(),
            &args.expected.text,
        ) {
            Ok(()) => Ok(json!({ "matched": true })),
            Err(err) => {
                if let Some(verify_err) = err.downcast_ref::<VerifyError>() {
                    return match verify_err {
                        VerifyError::ExpectationMismatch {
                            target_id,
                            expected,
                            actual,
                        } => Ok(json!({
                            "matched": false,
                            "target_id": target_id,
                            "expected": expected,
                            "actual": actual
                        })),
                    };
                }
                Err(err.context("verify tool failed"))
            }
        }
    }

    fn get_visual(&mut self, arguments: Value) -> Result<Value> {
        self.pending_visual_image = None;
        let args: GetVisualArguments =
            serde_json::from_value(arguments).context("invalid get_visual arguments")?;
        let capture = self.page.get_visual()?;

        let mut hasher = Sha256::new();
        hasher.update(&capture.image_png);
        let image_sha256 = hex::encode(hasher.finalize());

        let marks = if args.mode == "clean" {
            Vec::new()
        } else {
            capture
                .marks
                .into_iter()
                .map(|mark| {
                    json!({
                        "id": mark.id,
                        "stable_key": mark.stable_key.as_deref().map(shorten_key),
                        "bbox": mark.bbox
                    })
                })
                .collect::<Vec<_>>()
        };

        self.pending_visual_image = Some(capture.image_png);

        Ok(json!({
            "mode": args.mode,
            "viewport": args.viewport,
            "image_sha256": image_sha256,
            "marks": marks
        }))
    }

    fn ask_human(&mut self, arguments: Value) -> Result<Value> {
        let args: AskHumanArguments =
            serde_json::from_value(arguments).context("invalid ask_human arguments")?;

        let Some(pending) = self.page.pending_policy_approval() else {
            return Ok(json!({
                "approved": false,
                "reason": args.reason,
                "pending": false
            }));
        };

        self.page.approve_pending_policy_action()?;

        let mut payload = json!({
            "approved": true,
            "reason": args.reason,
            "pending": false,
            "rule_id": pending.rule_id,
            "scope": approval_scope_name(pending.scope)
        });

        if args.context {
            payload["context"] = json!({
                "action": pending.action,
                "target_signature": pending.target_signature
            });
        }

        Ok(payload)
    }

    fn run_skill(&mut self, arguments: Value) -> Result<Value> {
        let args: RunSkillArguments =
            serde_json::from_value(arguments).context("invalid run_skill arguments")?;

        self.page
            .log_skill_tool_call(&args.skill_name, &args.params);

        let Some(skill) = self.skills.get(&args.skill_name).cloned() else {
            return Ok(json!({
                "status": "not_found",
                "skill_name": args.skill_name
            }));
        };

        let mut runtime =
            PageSkillRuntime::new(&self.page, &args.params, &self.injection_sanitizer);
        let run_result = self
            .skill_engine
            .run(&skill, &mut runtime)
            .context("run_skill execution failed");
        // Always capture the delta (and, ISSUE-304, the accumulated security flags) so acts/
        // extracts that ran before a failure are not silently lost — same reasoning as the
        // delta capture already here (see the comment below), applied to `security_flags` too:
        // `into_usage_delta` consumes `runtime`, so anything else needed from it must be taken
        // first.
        let security_flags = std::mem::take(&mut runtime.security_flags);
        let delta = runtime.into_usage_delta();
        self.last_skill_delta = delta.clone();
        // ISSUE-301: any successful `act` step mutated the live page outside of
        // `CoreRuntimeBackend::act`'s bookkeeping. Invalidate before propagating `run_result`'s
        // error so a skill that fails partway through (after a successful action) still
        // discards the now-stale state/speculative bookkeeping.
        if delta.actions_executed > 0 {
            self.invalidate_after_opaque_skill_mutation();
        }
        let report = run_result?;

        // ISSUE-304: `report.outputs` already went through `injection_sanitizer` at the point
        // each value entered `ctx.extracted` (see `PageSkillRuntime::extract`) — apply PII
        // redaction here, at the external response boundary, matching the `extract` MCP tool's
        // own sanitize-then-redact order for page-derived content.
        let outputs: Value = report
            .outputs
            .into_iter()
            .map(|(key, value)| (key, core_runtime::privacy::global().redact_json(&value)))
            .collect::<serde_json::Map<String, Value>>()
            .into();
        // `message` needs the same PII redaction (Codex review): a `verify` step whose
        // `expected` came from `{{extracted.*}}` embeds both the extracted expected text and the
        // actual page text it was compared against directly into this failure message (see
        // `PageSession::verify_text`'s error) — unlike `outputs`, this text never passes through
        // `injection_sanitizer` either, so redact it here rather than widen `extract`'s
        // sanitization contract onto every step-failure message in this change.
        let message = report
            .message
            .as_deref()
            .map(|message| core_runtime::privacy::global().redact_text(message));

        Ok(json!({
            "status": skill_run_status_name(report.status),
            "message": message,
            "outputs": outputs,
            "security_flags": security_flags,
            "trace": report
                .trace
                .into_iter()
                .map(|entry| {
                    json!({
                        "step_id": entry.step_id,
                        "step_kind": entry.step_kind,
                        "operation": entry.operation,
                        "outcome": entry.outcome
                    })
                })
                .collect::<Vec<_>>()
        }))
    }

    fn extract(&mut self, arguments: Value) -> Result<Value> {
        let args: ExtractArguments =
            serde_json::from_value(arguments).context("invalid extract arguments")?;

        let rule = match (args.rule_name, args.inline) {
            (Some(name), _) => {
                let r = self.schema_registry.get(&name).ok_or_else(|| {
                    anyhow::anyhow!("extraction rule '{name}' not found in registry")
                })?;
                r.clone()
            }
            (None, Some(inline_val)) => ExtractionRule::from_value("inline", &inline_val)
                .context("failed to parse inline extraction rule")?,
            (None, None) => {
                anyhow::bail!("extract requires either 'rule_name' or 'inline'");
            }
        };

        // Use evaluate_script_json so arrays and objects are fully deserialized
        // (return_by_value: true), not returned as opaque remote handles. The checked script
        // surfaces JS exceptions (e.g. an invalid selector) that CDP would otherwise swallow.
        let envelope = self
            .page
            .evaluate_script_json(&rule.to_js_checked_script())
            .map_err(|err| {
                anyhow::anyhow!("ScriptEvalError: extraction script evaluation failed: {err:#}")
            })?;
        let raw_value = unwrap_extraction_envelope(envelope).map_err(|raw| {
            anyhow::anyhow!(
                "ScriptEvalError: {}",
                safe_script_error_message(&self.injection_sanitizer, &raw)
            )
        })?;
        let errors = if extraction_result_has_gaps(&raw_value) {
            self.diagnose_extraction(&rule)
        } else {
            serde_json::Map::new()
        };

        let (sanitized, mut security_flags) =
            self.injection_sanitizer.sanitize_json_value(raw_value);
        let (errors, error_flags) = sanitize_extraction_errors(&self.injection_sanitizer, errors);
        for flag in error_flags {
            if !security_flags.contains(&flag) {
                security_flags.push(flag);
            }
        }

        // Apply PII redaction before returning extracted content to the caller.
        let redacted = core_runtime::privacy::global().redact_json(&sanitized);

        let mut response = json!({
            "rule": rule.name,
            "result": redacted,
            "security_flags": security_flags
        });
        if !errors.is_empty() {
            response["errors"] = Value::Object(errors);
        }
        if args.debug {
            response["script"] = Value::String(rule.to_js_script());
        }
        Ok(response)
    }

    fn audit_retention_snapshot(&self) -> Option<AuditRetentionSnapshot> {
        // Prefer persistent sink metrics (storage-backed) when available.
        if let Some((retained_events, retained_bytes)) = self.page.persistent_audit_metrics() {
            return Some(AuditRetentionSnapshot {
                retained_events,
                retained_bytes,
            });
        }

        // Fall back to in-memory retained events when no persistent sink is configured.
        let events = self.page.audit_events();
        let retained_bytes = events
            .iter()
            .map(|event| {
                serde_json::to_vec(event)
                    .map(|bytes| bytes.len() as u64)
                    .unwrap_or_default()
            })
            .sum();

        Some(AuditRetentionSnapshot {
            retained_events: events.len() as u64,
            retained_bytes,
        })
    }

    fn take_skill_usage_delta(&mut self) -> SkillUsageDelta {
        std::mem::take(&mut self.last_skill_delta)
    }

    fn take_visual_image(&mut self) -> Option<Vec<u8>> {
        self.pending_visual_image.take()
    }

    fn handle_browser_disconnect(&mut self) -> std::result::Result<u64, String> {
        let Some(client) = self.client.as_mut() else {
            return Err("browser restart unavailable: no managed BrowserClient".to_string());
        };

        let now = Instant::now();
        while let Some(oldest) = self.restart_history.front() {
            if now.duration_since(*oldest) > RESTART_RATE_LIMIT_WINDOW {
                self.restart_history.pop_front();
            } else {
                break;
            }
        }
        if self.restart_history.len() >= RESTART_RATE_LIMIT_MAX {
            return Err(format!(
                "browser restart rate limit exceeded ({RESTART_RATE_LIMIT_MAX} restarts within \
                 {}s); the Chrome process may be crash-looping",
                RESTART_RATE_LIMIT_WINDOW.as_secs()
            ));
        }

        // Record this attempt before the fallible relaunch so that repeated
        // failures (e.g. a crash-looping Chrome that can't relaunch) are
        // still counted against the rate limit, not just successes.
        self.restart_history.push_back(now);

        let audit_logger = self.page.audit_logger_handle();
        let restart_count = self.browser_restarts + 1;
        let new_page = client
            .relaunch(audit_logger, "chrome process disconnected", restart_count)
            .map_err(|err| format!("relaunch failed: {err}"))?;

        if !self.policy_rules.is_empty() {
            new_page
                .set_policy_rules(self.policy_rules.clone())
                .map_err(|err| format!("failed to reapply policy rules after restart: {err}"))?;
        }

        self.page = Arc::new(new_page);
        self.state_cache = None;
        self.previous_semantic_state = None;
        self.previous_state_verified = true;
        // The new page is a fresh CDP session with new backend node IDs and
        // a new `page_instance_id` (Spec §3.5 / ISSUE-147 review): any
        // pending speculative bookkeeping and cached snapshots refer to the
        // crashed page and must not be reused or trained against.
        self.pending_action = None;
        self.last_action = None;
        self.last_served_prediction = None;
        self.last_served_prediction_source_hash = None;
        self.action_chain_broken = false;
        self.speculative.reset_session();
        self.browser_restarts = restart_count;
        Ok(self.browser_restarts)
    }

    fn browser_restart_count(&self) -> u64 {
        self.browser_restarts
    }

    fn is_chrome_process_alive(&self) -> Option<bool> {
        self.client.as_ref()?.is_process_alive()
    }

    fn confirm_browser_disconnected(&self) -> bool {
        match self.client.as_ref() {
            // No managed BrowserClient (e.g. backends constructed via
            // `CoreRuntimeBackend::new` for tests) cannot run a liveness
            // probe; preserve the prior always-restart-on-marker-match
            // behavior.
            None => true,
            Some(client) => !client.confirm_alive(core_runtime::HEALTH_CHECK_TIMEOUT),
        }
    }

    fn speculative_usage(&self) -> (u64, u64) {
        (self.speculative_hits, self.speculative_misses)
    }
}
