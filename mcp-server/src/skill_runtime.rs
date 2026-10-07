use super::*;

pub(crate) struct PageSkillRuntime<'a> {
    pub(crate) page: &'a PageSession,
    pub(crate) params: &'a Value,
    pub(crate) injection_sanitizer: &'a PromptInjectionSanitizer,
    pub(crate) actions_executed: u64,
    /// Reserved for when a `get_visual` skill step type is introduced; not yet incremented.
    pub(crate) visual_captures: u64,
    /// Reserved for when a skill-internal HITL step type is introduced; not yet incremented.
    pub(crate) hitl_events: u64,
    /// Prompt-injection security flags accumulated from every `extract` step's sanitization
    /// (ISSUE-304) — surfaced in `run_skill`'s response alongside `outputs` so a caller can tell
    /// sanitized-but-still-suspicious page content apart from ordinary text, the same signal
    /// the top-level `extract` MCP tool already gives.
    pub(crate) security_flags: Vec<String>,
}

impl<'a> PageSkillRuntime<'a> {
    pub(crate) fn new(
        page: &'a PageSession,
        params: &'a Value,
        injection_sanitizer: &'a PromptInjectionSanitizer,
    ) -> Self {
        Self {
            page,
            params,
            injection_sanitizer,
            actions_executed: 0,
            visual_captures: 0,
            hitl_events: 0,
            security_flags: Vec::new(),
        }
    }

    pub(crate) fn into_usage_delta(self) -> SkillUsageDelta {
        SkillUsageDelta {
            actions_executed: self.actions_executed,
            visual_captures: self.visual_captures,
            hitl_events: self.hitl_events,
        }
    }
}

/// Resolves `template` inside a `resolve_template` call and returns early with
/// `OperationOutcome::Failure` on error — `resolve_template` returns `Result` (ISSUE-304), but
/// every `SkillRuntime` trait method here returns `OperationOutcome` directly and so can't use
/// `?`.
macro_rules! resolve_or_fail {
    ($template:expr, $params:expr, $ctx:expr, $slot:expr) => {
        match resolve_template($template, $params, $ctx, $slot) {
            Ok(value) => value,
            Err(reason) => return OperationOutcome::Failure { reason },
        }
    };
}

impl SkillRuntime for PageSkillRuntime<'_> {
    fn locate(
        &mut self,
        step: &LocateStep,
        ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        let query = resolve_or_fail!(&step.query, self.params, ctx, TemplateSlot::Control);
        if let Some(id) = parse_target_id(&query) {
            return match self.page.capture_semantic_state(LoadProfile::Interactive) {
                Ok(state) => {
                    if node_exists_by_id(state.root(), id) {
                        OperationOutcome::Success
                    } else {
                        OperationOutcome::Failure {
                            reason: format!("element id:{id} not found in current state"),
                        }
                    }
                }
                Err(err) => OperationOutcome::Failure {
                    reason: err.to_string(),
                },
            };
        }
        if let Some(key) = parse_target_stable_key(&query) {
            // Refresh the SRE cache before lookup so we don't read stale state.
            if let Err(err) = self.page.capture_semantic_state(LoadProfile::Interactive) {
                return OperationOutcome::Failure {
                    reason: err.to_string(),
                };
            }
            return if self
                .page
                .lookup_backend_node_id_by_stable_key(&key)
                .is_some()
            {
                OperationOutcome::Success
            } else {
                OperationOutcome::Failure {
                    reason: format!("element stable_key:{key} not found"),
                }
            };
        }
        OperationOutcome::Failure {
            reason: format!(
                "unrecognised locate query {query:?}; expected id:<N> or stable_key:<key>"
            ),
        }
    }

    fn verify(
        &mut self,
        step: &VerifyStep,
        ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        let target = resolve_or_fail!(&step.target, self.params, ctx, TemplateSlot::Control);
        // `expected` is compared against page text, not used to choose a target/selector, so an
        // extracted value is safe here (e.g. verifying a confirmation banner echoes an id a
        // prior step extracted).
        let expected = resolve_or_fail!(&step.expected, self.params, ctx, TemplateSlot::Data);
        if let Some(id) = parse_target_id(&target) {
            let stable_key = parse_target_stable_key(&target);
            return match self.page.verify_text(id, stable_key.as_deref(), &expected) {
                Ok(()) => OperationOutcome::Success,
                Err(err) => OperationOutcome::Failure {
                    reason: err.to_string(),
                },
            };
        }
        if let Some(key) = parse_target_stable_key(&target) {
            // Refresh the SRE cache before lookup so we don't read stale state.
            if let Err(err) = self.page.capture_semantic_state(LoadProfile::Interactive) {
                return OperationOutcome::Failure {
                    reason: err.to_string(),
                };
            }
            let Some(id) = self.page.lookup_backend_node_id_by_stable_key(&key) else {
                return OperationOutcome::Failure {
                    reason: format!("element stable_key:{key} not found"),
                };
            };
            return match self.page.verify_text(id, Some(&key), &expected) {
                Ok(()) => OperationOutcome::Success,
                Err(err) => OperationOutcome::Failure {
                    reason: err.to_string(),
                },
            };
        }
        OperationOutcome::Failure {
            reason: format!(
                "unrecognised verify target {target:?}; expected id:<N> or stable_key:<key>"
            ),
        }
    }

    fn act(
        &mut self,
        step: &ActStep,
        ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        // `target`/`action` choose *what element* and *which verb* execute — page content must
        // never control either, so both reject `{{extracted.*}}` (ISSUE-304 review). Only
        // `value` (the data typed/selected) may come from an extraction.
        let target = resolve_or_fail!(&step.target, self.params, ctx, TemplateSlot::Control);
        let action = resolve_or_fail!(&step.action, self.params, ctx, TemplateSlot::Control);
        let value = match step.value.as_deref() {
            Some(raw) => Some(resolve_or_fail!(raw, self.params, ctx, TemplateSlot::Data)),
            None => None,
        };

        let target_id = parse_target_id(&target);
        let target_stable_key = parse_target_stable_key(&target);

        match self.page.act(
            target_id,
            target_stable_key.as_deref(),
            &action,
            value.as_deref(),
        ) {
            Ok(()) => {
                self.actions_executed += 1;
                OperationOutcome::Success
            }
            Err(err) => OperationOutcome::Failure {
                reason: err.to_string(),
            },
        }
    }

    fn wait(
        &mut self,
        step: &WaitStep,
        ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        let condition = resolve_or_fail!(&step.condition, self.params, ctx, TemplateSlot::Control);
        if let Some(intent) = condition.strip_prefix("intent:") {
            return match self
                .page
                .wait_for_intent(intent.trim(), Duration::from_millis(step.timeout_ms))
            {
                Ok(()) => OperationOutcome::Success,
                Err(err) => OperationOutcome::Failure {
                    reason: err.to_string(),
                },
            };
        }
        if let Some((target, desired_state)) = parse_semantic_wait_condition(&condition) {
            return match self.page.wait_for_semantic(
                target,
                desired_state,
                Duration::from_millis(step.timeout_ms),
            ) {
                Ok(()) => OperationOutcome::Success,
                Err(err) => OperationOutcome::Failure {
                    reason: err.to_string(),
                },
            };
        }
        OperationOutcome::Failure {
            reason: format!(
                "unrecognised wait condition {condition:?}; expected intent:<text> or id:<N>:enabled"
            ),
        }
    }

    fn extract(
        &mut self,
        step: &ExtractStep,
        ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        let selector = resolve_or_fail!(&step.selector, self.params, ctx, TemplateSlot::Control);
        let script = format!(
            "(() => {{ const el = document.querySelector({}); return el ? (el.innerText || el.textContent || '').trim() : null; }})()",
            serde_json::to_string(&selector).unwrap_or_else(|_| "\"\"".to_string())
        );

        match self.page.evaluate_script(&script) {
            Ok(object) => {
                // A missing element stores `Value::Null`, distinct from a successful-but-empty
                // extraction (an empty string) — see `resolve_extracted_scalar`, which rejects
                // `Null` when a later template references `{{extracted.<key>}}`, so a failed
                // extraction fails that step loudly instead of silently typing an empty value
                // (ISSUE-304 review).
                let raw = object.value.unwrap_or(Value::Null);
                // Extracted content is untrusted page content (ISSUE-304 review): sanitize it
                // here, at the point it enters `ctx.extracted`, so *every* later consumer — a
                // template substitution into another step, or `run_skill`'s `outputs` — sees the
                // sanitized form. Sanitizing only at the `run_skill` output boundary would leave
                // the far more dangerous path (an unsanitized extracted value flowing into a
                // later `act`) unprotected.
                let (sanitized, flags) = self.injection_sanitizer.sanitize_json_value(raw);
                self.security_flags.extend(flags);
                ctx.extracted.insert(step.key.clone(), sanitized);
                OperationOutcome::Success
            }
            Err(err) => OperationOutcome::Failure {
                reason: err.to_string(),
            },
        }
    }
}

/// Whether a template's resolved value is used as *data* or as *control* (ISSUE-304).
#[derive(Clone, Copy)]
pub(crate) enum TemplateSlot {
    /// The resolved value is data: typed text, a comparison string. A page-derived
    /// `{{extracted.*}}` value is acceptable here.
    Data,
    /// The resolved value chooses *what* to act on or *how* — a target, a selector, an action
    /// verb, a wait condition. Page content must never be allowed to control these, so
    /// `{{extracted.*}}` is rejected here even though the syntax is otherwise valid.
    Control,
}

/// Resolves a skill step's `{{...}}` template against `params` (the `run_skill` invocation
/// arguments) and/or `ctx.extracted` (values produced by prior `extract` steps), per ISSUE-304.
///
/// Three distinct forms, deliberately non-overlapping so there is never a precedence question:
/// - `{{params.KEY}}` — an invocation parameter. Missing or non-scalar (`KEY` not present, or
///   not a string/number/bool) is an `Err` — unlike the legacy form below, nothing here existed
///   before ISSUE-304, so there is no backward-compatibility reason to resolve it silently.
/// - `{{extracted.KEY}}` — a value a prior `extract` step produced. Rejected outright (`Err`) in
///   a `TemplateSlot::Control` position. In a `TemplateSlot::Data` position: missing (no such key
///   extracted yet, or ever) or non-scalar (in particular `Value::Null`, which is how a
///   selector-miss extraction is stored — see `PageSkillRuntime::extract`) is also an `Err`, so a
///   failed/absent extraction fails the referencing step loudly instead of silently substituting
///   an empty string or the literal text `null`.
/// - Bare `{{KEY}}` (no `params.`/`extracted.` prefix) — the original, pre-ISSUE-304 behavior,
///   kept byte-for-byte compatible: resolves against `params` only, and a miss or non-scalar
///   value falls back to returning the literal template text unchanged (e.g.
///   `examples/sample_skill.json`'s `"value": "{{email}}"` must keep working exactly as before).
///
/// Text that isn't wrapped in `{{...}}` at all is returned unchanged, always `Ok`.
pub(crate) fn resolve_template(
    template: &str,
    params: &Value,
    ctx: &skills_engine::SkillExecutionContext,
    slot: TemplateSlot,
) -> Result<String, String> {
    let trimmed = template.trim();
    let Some(inner) = trimmed
        .strip_prefix("{{")
        .and_then(|rest| rest.strip_suffix("}}"))
        .map(str::trim)
    else {
        return Ok(template.to_string());
    };

    if let Some(key) = inner.strip_prefix("params.").map(str::trim) {
        return resolve_scalar(params.get(key))
            .ok_or_else(|| format!("skill template references undefined params.{key}"));
    }

    if let Some(key) = inner.strip_prefix("extracted.").map(str::trim) {
        if matches!(slot, TemplateSlot::Control) {
            return Err(format!(
                "template {{{{extracted.{key}}}}} is not allowed here — this field selects an \
                 action/target/selector and must not be derived from extracted page content"
            ));
        }
        return resolve_scalar(ctx.extracted.get(key)).ok_or_else(|| {
            format!(
                "skill template references undefined or non-scalar extracted.{key} — either no \
                 prior extract step produced this key, it runs later in execution order, or the \
                 extraction found no matching element (stored as null)"
            )
        });
    }

    Ok(resolve_scalar(params.get(inner)).unwrap_or_else(|| template.to_string()))
}

/// A string/number/bool value stringifies; anything else (missing, `null`, array, object) is
/// `None` — templates only ever substitute scalar values.
pub(crate) fn resolve_scalar(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(s) = value.as_str() {
        return Some(s.to_string());
    }
    if value.is_number() || value.is_boolean() {
        return Some(value.to_string());
    }
    None
}
