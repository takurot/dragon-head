use super::*;
use std::ops::ControlFlow;

impl PageSession {
    /// Resolve the element bounding box `[x, y, width, height]` from a backend node id.
    pub fn get_element_bbox(&self, backend_node_id: i64) -> Result<Option<[f64; 4]>> {
        self.resolve_node_bbox(backend_node_id)
    }

    /// Verify element text against an expected value.
    /// On mismatch, triggers a SoM capture to help disambiguate recovery.
    ///
    /// If `target_id` does not resolve (e.g. a stale id reported by a
    /// speculative `get_state` response, Spec §3.5 / ISSUE-147), falls back
    /// to `stable_key`: refreshes the stable-key index from the live DOM and
    /// retries against the resolved id, mirroring `act`'s recovery path.
    pub fn verify_text(
        &self,
        target_id: i64,
        stable_key: Option<&str>,
        expected_text: &str,
    ) -> Result<()> {
        self.audit_logger.log(AuditEvent::ToolCall {
            tool_name: "verify_text".to_string(),
            args: serde_json::json!({
                "target_id": target_id,
                "stable_key": stable_key,
                "expected_text": expected_text
            }),
            timestamp: epoch_millis_u64(),
        });

        let (resolved_id, actual_text) = match self.get_element_text(target_id) {
            Ok(text) => (target_id, text),
            Err(e) => {
                let node_error_markers = [
                    "could not find node",
                    "no node with given id",
                    "could not find object with given id",
                    "no object with given id",
                    "node does not exist",
                ];
                if !error_chain_contains_any(&e, &node_error_markers) {
                    return Err(e);
                }
                let Some(key) = stable_key else {
                    return Err(e);
                };
                self.refresh_stable_key_index(crate::sre::LoadProfile::Interactive)?;
                let Some(new_id) = self.lookup_backend_node_id_by_stable_key(key) else {
                    return Err(e);
                };
                let text = self.get_element_text(new_id)?;
                (new_id, text)
            }
        };

        if normalize_text(&actual_text) == normalize_text(expected_text) {
            return Ok(());
        }

        self.trigger_som_capture_best_effort(SomTrigger::VerifyFailed);
        Err(VerifyError::ExpectationMismatch {
            target_id: resolved_id,
            expected: expected_text.to_string(),
            actual: actual_text,
        }
        .into())
    }

    /// Perform an action on a target element.
    /// Uses `target_id` (backend_node_id) preferentially.
    /// If `target_id` is invalid (stale), attempts fallback using `stable_key` by re-scanning the DOM.
    pub fn act(
        &self,
        target_id: Option<i64>,
        stable_key: Option<&str>,
        action: &str,
        value: Option<&str>,
    ) -> Result<()> {
        self.audit_logger.log(AuditEvent::ToolCall {
            tool_name: "act".to_string(),
            args: serde_json::json!({
                "target_id": target_id,
                "stable_key": stable_key,
                "action": action,
                "value": value
            }),
            timestamp: epoch_millis_u64(),
        });

        self.enforce_policy(target_id, stable_key, action)?;

        if let Some(bid) = target_id {
            if let ControlFlow::Break(result) =
                self.act_by_target_id(bid, stable_key, action, value)
            {
                return result;
            }
        }

        if let Some(key) = stable_key {
            return self.act_by_stable_key(key, target_id, action, value);
        }

        self.fail_unresolved_act(
            action,
            target_id,
            stable_key,
            "neither target_id nor stable_key resolved a target",
        )
    }

    /// First `act` attempt, using the caller's `target_id`. `Break` carries the final result
    /// (success, a non-node error, or an unresolved target with no `stable_key` to fall back
    /// on); `Continue` means the node was stale and the `stable_key` fallback should run.
    fn act_by_target_id(
        &self,
        bid: i64,
        stable_key: Option<&str>,
        action: &str,
        value: Option<&str>,
    ) -> ControlFlow<Result<()>> {
        match self.perform_action_by_id(bid, action, value) {
            Ok(_) => {
                // Seed the DOMSignatureCache so future stale-key recovery can use it.
                if let Some(key) = stable_key {
                    self.record_dom_signature_for_node_id(key, bid);
                }
                ControlFlow::Break(Ok(()))
            }
            Err(e) => {
                // Only fallback if the error indicates a node issue (e.g. "Could not find node", "No node with given id")
                // If it's a timeout or other error, we probably shouldn't blindly retry?
                // The CDP error for invalid backend_node_id usually says "Could not find node with given id".
                let node_error_markers = [
                    "could not find node",
                    "no node with given id",
                    "could not find object with given id",
                    "no object with given id",
                    "node does not exist",
                ];
                if !error_chain_contains_any(&e, &node_error_markers) {
                    return ControlFlow::Break(Err(e));
                }

                if stable_key.is_none() {
                    return ControlFlow::Break(self.fail_unresolved_act(
                        action,
                        Some(bid),
                        stable_key,
                        "target_id lookup failed and no stable_key was provided",
                    ));
                }
                ControlFlow::Continue(())
            }
        }
    }

    /// `act` fallback when `target_id` was missing or stale: re-resolve by `stable_key`, then by
    /// self-healing recovery (PR-21), and finally ask a human.
    fn act_by_stable_key(
        &self,
        key: &str,
        target_id: Option<i64>,
        action: &str,
        value: Option<&str>,
    ) -> Result<()> {
        // Refresh semantic snapshot and stable-key index before lookup.
        self.capture_state(crate::sre::LoadProfile::Interactive)?;

        if let Some(new_id) = self.lookup_backend_node_id_by_stable_key(key) {
            self.record_action_log(
                "warning",
                "stable_key_fallback_recovered",
                action,
                target_id,
                Some(key),
                &format!("Action recovered via stable key: {key} -> new_id: {new_id}"),
            );
            // Record a fresh signature for the successfully located node.
            self.record_dom_signature_for_node_id(key, new_id);
            return self.perform_action_by_id(new_id, action, value);
        }

        // Both target_id and stable_key lookup failed →
        // attempt Self-Healing Context Recovery (PR-21).
        if let Some(recovered_id) = self.try_self_healing_recovery(key) {
            // Re-enforce policy against the recovered node so target-text /
            // surrounding-context rules are evaluated on the actual target.
            self.enforce_policy(Some(recovered_id), Some(key), action)?;
            return self.perform_action_by_id(recovered_id, action, value);
        }

        // Recovery failed → AskHumanRequired fallback.
        self.record_action_log(
            "error",
            "ask_human_required",
            action,
            target_id,
            Some(key),
            &format!(
                "Self-healing recovery failed for stable_key={key}; human intervention required"
            ),
        );
        self.trigger_som_capture_best_effort(SomTrigger::ActAmbiguous);
        Err(ActionError::AskHumanRequired {
            reason: format!(
                "target_id and stable_key both failed for key={key}; \
                 self-healing found no confident match"
            ),
        }
        .into())
    }

    /// Logs and returns `VerifyRequired` for an `act` whose target could not be resolved.
    fn fail_unresolved_act(
        &self,
        action: &str,
        target_id: Option<i64>,
        stable_key: Option<&str>,
        detail: &str,
    ) -> Result<()> {
        self.record_action_log(
            "error",
            "verify_required",
            action,
            target_id,
            stable_key,
            detail,
        );
        self.trigger_som_capture_best_effort(SomTrigger::ActAmbiguous);
        Err(ActionError::VerifyRequired.into())
    }

    // -------------------------------------------------------------------------
    // Self-Healing Context Recovery helpers (PR-21 / ISSUE-11)
    // -------------------------------------------------------------------------

    /// Attempt to recover a `backend_node_id` via `DOMSignatureCache` fuzzy
    /// matching when both `target_id` and `stable_key` index lookup have failed.
    ///
    /// On a confident match the cache is updated with the new `stable_key`→node
    /// mapping (learning), and the `backend_node_id` of the matched node is
    /// returned.  Returns `None` if no candidate exceeds the recovery threshold.
    pub(super) fn try_self_healing_recovery(&self, stable_key: &str) -> Option<i64> {
        // Collect all interactive nodes from the current semantic snapshot.
        let state = self
            .semantic_capture_cache
            .lock()
            .ok()
            .and_then(|g| g.last_state.clone())?;

        let candidates = Self::collect_interactive_nodes(state.root());

        let matched = self
            .dom_signature_cache
            .find_best_match(stable_key, &candidates)?;

        let new_id = matched.backend_node_id;
        if new_id == 0 {
            return None;
        }

        // Learning: update cache with the newly discovered node.
        self.dom_signature_cache.record(stable_key, matched);

        // Redact label before logging to avoid PII leaking via action logs.
        let redacted_label = matched
            .label
            .as_deref()
            .map(|l| crate::privacy::global().redact_text(l));
        self.record_action_log(
            "warning",
            "self_healing_recovered",
            "self_healing",
            None,
            Some(stable_key),
            &format!(
                "Self-Healing: fuzzy match recovered stable_key={stable_key} \
                 to backend_node_id={new_id} (role={}, label={:?})",
                matched.role, redacted_label,
            ),
        );

        Some(new_id)
    }

    /// Record a `NodeSignature` in the `DOMSignatureCache` for the node
    /// identified by `backend_node_id` in the current stable_key index.
    pub(super) fn record_dom_signature_for_node_id(&self, stable_key: &str, backend_node_id: i64) {
        // Walk the current semantic snapshot to find the node.
        let Some(state) = self
            .semantic_capture_cache
            .lock()
            .ok()
            .and_then(|g| g.last_state.clone())
        else {
            return;
        };

        fn find_node(root: &SemanticNode, id: i64) -> Option<&SemanticNode> {
            if root.backend_node_id == id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_node(c, id))
        }

        if let Some(node) = find_node(state.root(), backend_node_id) {
            self.dom_signature_cache.record(stable_key, node);
        }
    }

    /// Collect all interactive leaf nodes from a DOM tree (flat, depth-first).
    pub(super) fn collect_interactive_nodes(root: &SemanticNode) -> Vec<SemanticNode> {
        let mut out = Vec::new();
        Self::collect_nodes_recursive(root, &mut out);
        out
    }

    pub(super) fn collect_nodes_recursive(node: &SemanticNode, out: &mut Vec<SemanticNode>) {
        // Include nodes that have a non-zero backend_node_id (live DOM elements).
        if node.backend_node_id != 0 {
            out.push(node.clone());
        }
        for child in &node.children {
            Self::collect_nodes_recursive(child, out);
        }
    }

    pub(super) fn resolve_node_bbox(&self, backend_node_id: i64) -> Result<Option<[f64; 4]>> {
        let node_id_u32 =
            u32::try_from(backend_node_id).context("Invalid backend_node_id: must fit in u32")?;

        let result = self
            .inner
            .call_method(headless_chrome::protocol::cdp::DOM::GetBoxModel {
                node_id: None,
                backend_node_id: Some(node_id_u32),
                object_id: None,
            });

        let model = match result {
            Ok(response) => response.model,
            Err(err) => {
                // A speculatively-served snapshot (Spec §3.5 / ISSUE-147) may
                // reference a `backend_node_id` from a prior DOM that no
                // longer exists. CDP reports this as an error rather than an
                // empty box model; treat it the same as "no box" (`None`).
                let node_error_markers = [
                    "could not find node",
                    "no node with given id",
                    "could not compute box model",
                    "node does not exist",
                ];
                if error_chain_contains_any(&err, &node_error_markers) {
                    return Ok(None);
                }
                return Err(err).context("Failed to get box model");
            }
        };

        Ok(quad_to_bbox(&model.content))
    }

    pub(super) fn get_element_text(&self, backend_node_id: i64) -> Result<String> {
        use headless_chrome::protocol::cdp::Runtime::CallFunctionOn;

        let object_id = self.resolve_node_object_id(backend_node_id)?;
        let result = self
            .inner
            .call_method(CallFunctionOn {
                object_id: Some(object_id),
                function_declaration:
                    "function() { return (this.innerText || this.textContent || '').trim(); }"
                        .to_string(),
                arguments: None,
                silent: Some(true),
                return_by_value: Some(true),
                generate_preview: Some(false),
                user_gesture: Some(false),
                await_promise: Some(false),
                execution_context_id: None,
                object_group: None,
                throw_on_side_effect: None,
                unique_context_id: None,
                serialization_options: None,
            })
            .context("Failed to extract element text")?;

        let value = result
            .result
            .value
            .unwrap_or_else(|| serde_json::Value::String(String::new()));

        if let Some(text) = value.as_str() {
            Ok(text.to_string())
        } else {
            Ok(value.to_string())
        }
    }

    pub(super) fn resolve_node_object_id(&self, backend_node_id: i64) -> Result<String> {
        use headless_chrome::protocol::cdp::DOM::ResolveNode;

        let node_id_u32 =
            u32::try_from(backend_node_id).context("Invalid backend_node_id: must fit in u32")?;

        let remote_object = self
            .inner
            .call_method(ResolveNode {
                node_id: None,
                backend_node_id: Some(node_id_u32),
                object_group: None,
                execution_context_id: None,
            })
            .context("Failed to resolve node")?
            .object;

        remote_object
            .object_id
            .context("Failed to resolve node to object")
    }

    pub(super) fn perform_action_by_id(
        &self,
        backend_node_id: i64,
        action: &str,
        value: Option<&str>,
    ) -> Result<()> {
        // Resolve backend_node_id to RemoteObject
        use headless_chrome::protocol::cdp::Runtime::CallFunctionOn;
        let object_id = self.resolve_node_object_id(backend_node_id)?;

        match action {
            "click" => {
                self.inner.call_method(CallFunctionOn {
                    object_id: Some(object_id),
                    function_declaration: "function() { this.click(); }".to_string(),
                    arguments: None,
                    silent: Some(true),
                    return_by_value: Some(false),
                    generate_preview: Some(false),
                    user_gesture: Some(true),
                    await_promise: Some(false),
                    execution_context_id: None,
                    object_group: None,
                    throw_on_side_effect: None,
                    unique_context_id: None,
                    serialization_options: None,
                })?;
            }
            "type" => {
                let text = value.context("Value is required for type action")?;
                // Focus then type
                self.inner.call_method(CallFunctionOn {
                    object_id: Some(object_id.clone()),
                    function_declaration: "function() { this.focus(); }".to_string(),
                    arguments: None,
                    silent: Some(true),
                    return_by_value: Some(false),
                    generate_preview: Some(false),
                    user_gesture: Some(true),
                    await_promise: Some(false),
                    execution_context_id: None,
                    object_group: None,
                    throw_on_side_effect: None,
                    unique_context_id: None,
                    serialization_options: None,
                })?;
                self.inner.type_str(text)?;
            }
            _ => anyhow::bail!("Unsupported action: {}", action),
        }
        Ok(())
    }
}
