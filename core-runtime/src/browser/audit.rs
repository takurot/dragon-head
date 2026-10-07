use super::*;

impl PageSession {
    pub fn action_logs(&self) -> Result<Vec<ActionLogEntry>> {
        let guard = self
            .action_logs
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock action logs"))?;
        Ok(guard.iter().cloned().collect())
    }

    pub fn audit_events(&self) -> Vec<AuditEvent> {
        self.audit_logger.recent_events()
    }

    pub fn clear_audit_events(&self) {
        self.audit_logger.clear_recent_events();
    }

    /// Returns `(events_written, bytes_written)` from the persistent rolling-file sink,
    /// or `None` when no persistent sink is configured (e.g. `AUDIT_LOG_DIR` is unset).
    pub fn persistent_audit_metrics(&self) -> Option<(u64, u64)> {
        self.audit_logger.persistent_metrics()
    }

    /// Returns a clone of this session's [`AuditLogger`] handle.
    ///
    /// Used to carry the same audit logger (and its persistent sink) into the
    /// fresh [`PageSession`] created by [`BrowserClient::relaunch`] (ISSUE-149).
    pub fn audit_logger_handle(&self) -> AuditLogger {
        (*self.audit_logger).clone()
    }

    /// Record a top-level `TOOL_CALL` audit event for a `run_skill` request.
    ///
    /// Mirrors the `act`/`verify_text` tool-call logging so every `run_skill`
    /// request is captured in the audit trail per Spec §AUD-01, even when the
    /// skill's steps never reach an `act` call or the skill fails outright
    /// (ISSUE-187).
    pub fn log_skill_tool_call(&self, skill_name: &str, params: &serde_json::Value) {
        self.audit_logger.log(AuditEvent::ToolCall {
            tool_name: "run_skill".to_string(),
            args: serde_json::json!({
                "skill_name": skill_name,
                "params": params
            }),
            timestamp: epoch_millis_u64(),
        });
    }

    // -------------------------------------------------------------------------

    pub(super) fn record_action_log(
        &self,
        level: &str,
        code: &str,
        action: &str,
        target_id: Option<i64>,
        stable_key: Option<&str>,
        message: &str,
    ) {
        let entry = ActionLogEntry {
            level: level.to_string(),
            code: code.to_string(),
            action: action.to_string(),
            target_id,
            stable_key: stable_key.map(|key| key.to_string()),
            message: message.to_string(),
            timestamp: epoch_millis_u64(),
        };

        if let Ok(mut guard) = self.action_logs.lock() {
            guard.push_back(entry.clone());
            while guard.len() > ACTION_LOG_BUFFER_LIMIT {
                guard.pop_front();
            }
        } else {
            tracing::error!("failed to lock structured action log buffer");
        }

        // `level` is caller-chosen ("error"/"warning" today) and `tracing`'s macros need a
        // compile-time level, so dispatch explicitly rather than trying to make the level
        // dynamic — an unrecognized value logs at `info` rather than being dropped.
        macro_rules! log_action_event {
            ($lvl:ident) => {
                tracing::$lvl!(
                    code = entry.code,
                    action = entry.action,
                    target_id = entry.target_id,
                    stable_key = entry.stable_key.as_deref(),
                    message = entry.message,
                    "action log"
                )
            };
        }
        match level {
            "error" => log_action_event!(error),
            "warning" => log_action_event!(warn),
            _ => log_action_event!(info),
        }
    }
}
