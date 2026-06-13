use crate::audit_sink::{AuditSink, DurabilityMode, MeteredSink, RollingFileSink};
use crate::privacy;
use crate::sre::SemanticState;
use crossbeam_channel::{unbounded, Sender};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    env,
    sync::{Arc, Mutex},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum AuditEvent {
    #[serde(rename = "STATE_SNAPSHOT")]
    StateSnapshot {
        state_hash: String,
        page_instance_id: String,
        timestamp: u64,
        payload: serde_json::Value,
    },
    #[serde(rename = "STATE_PATCH")]
    StatePatch {
        state_hash: String,
        page_instance_id: String,
        timestamp: u64,
        patch: serde_json::Value,
    },
    #[serde(rename = "TOOL_CALL")]
    ToolCall {
        tool_name: String,
        args: serde_json::Value,
        timestamp: u64,
    },
    #[serde(rename = "POLICY_DECISION")]
    PolicyDecision {
        rule_id: String,
        action: String,
        decision: String,
        timestamp: u64,
    },
    #[serde(rename = "HITL_EVENT")]
    HitlEvent {
        event_type: String,
        reason: Option<String>,
        user_id: Option<String>,
        timestamp: u64,
    },
    #[serde(rename = "VISUAL_CAPTURE")]
    VisualCapture {
        trigger: String,
        marks_count: usize,
        timestamp: u64,
    },
    #[serde(rename = "PLUGIN_STATE_TRANSFORM")]
    PluginStateTransform {
        plugin_id: String,
        success: bool,
        error_message: Option<String>,
        timestamp: u64,
    },
    #[serde(rename = "PLUGIN_POLICY_DECISION")]
    PluginPolicyDecision {
        plugin_id: String,
        allowed: bool,
        reason: Option<String>,
        timestamp: u64,
    },
    /// Emitted when the Chrome process disconnected and the session was
    /// automatically relaunched with a fresh page (ISSUE-149).
    #[serde(rename = "BROWSER_RESTART")]
    BrowserRestart {
        reason: String,
        restart_count: u64,
        timestamp: u64,
    },
}

/// Handle to the metrics of a persistent `MeteredSink<RollingFileSink>`.
///
/// Clone is cheap (`Arc` clone). Metrics are updated atomically by the audit worker thread.
#[derive(Clone)]
pub struct PersistentSinkHandle(Arc<MeteredSink<RollingFileSink>>);

impl PersistentSinkHandle {
    /// Events successfully written to the persistent sink.
    pub fn events_written(&self) -> u64 {
        self.0.events_written()
    }

    /// Serialized bytes successfully written to the persistent sink.
    pub fn bytes_written(&self) -> u64 {
        self.0.bytes_written()
    }
}

#[derive(Clone)]
pub struct AuditLogger {
    sender: Sender<AuditMessage>,
    recent_events: Arc<Mutex<VecDeque<AuditEvent>>>,
    /// Present only when a persistent rolling-file sink was configured.
    persistent: Option<PersistentSinkHandle>,
}

enum AuditMessage {
    Event(AuditEvent),
    StateUpdate {
        previous: Option<Arc<SemanticState>>,
        current: Arc<SemanticState>,
    },
}

impl Default for AuditLogger {
    fn default() -> Self {
        Self::new()
    }
}

impl AuditLogger {
    /// Lightweight logger with no persistent sink.  Suitable for tests and development.
    pub fn new() -> Self {
        Self::with_sinks(Vec::new(), None)
    }

    /// Production constructor that reads configuration from environment variables:
    ///
    /// | Variable              | Default          | Description                                         |
    /// |-----------------------|------------------|-----------------------------------------------------|
    /// | `AUDIT_LOG_DIR`       | *(unset)*        | Directory for rolling NDJSON files. If unset, no persistent sink is created. |
    /// | `AUDIT_LOG_MAX_BYTES` | `10485760` (10 MiB) | Rotate log file after this many bytes. `0` = never rotate. |
    /// | `AUDIT_DURABILITY`    | `flush`          | `flush` or `sync`. Use `sync` for crash-safe retention. |
    ///
    /// When `AUDIT_LOG_DIR` is unset the logger behaves identically to [`AuditLogger::new()`].
    /// Construction errors (bad dir, permission denied) are logged to stderr and fall back to
    /// no-persistent-sink mode so the production binary never panics on audit misconfiguration.
    pub fn from_env() -> Self {
        Self::from_env_with(|key| env::var(key).ok())
    }

    /// Testable variant of [`from_env`] — accepts an env-lookup closure instead of reading
    /// `std::env` directly.  This avoids `set_var`/`remove_var` in tests, which are unsound
    /// under parallel test runners (project rule #11).
    pub fn from_env_with(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let Some(dir) = lookup("AUDIT_LOG_DIR").filter(|s| !s.is_empty()) else {
            return Self::new();
        };

        let max_bytes: u64 = lookup("AUDIT_LOG_MAX_BYTES")
            .and_then(|s| {
                s.parse().ok().or_else(|| {
                    eprintln!(
                        "[AUDIT][WARN] AUDIT_LOG_MAX_BYTES='{s}' is not a valid integer; using 10 MiB default."
                    );
                    None
                })
            })
            .unwrap_or(10 * 1024 * 1024); // 10 MiB default

        let durability = match lookup("AUDIT_DURABILITY").as_deref() {
            Some("sync") => DurabilityMode::Sync,
            _ => DurabilityMode::Flush,
        };

        let sink = match RollingFileSink::new(&dir, "audit", max_bytes) {
            Ok(s) => s.with_durability(durability),
            Err(e) => {
                eprintln!(
                    "[AUDIT][ERROR] Failed to create persistent sink at '{dir}': {e}. Falling back to in-memory only."
                );
                return Self::new();
            }
        };

        let metered = Arc::new(MeteredSink::new(sink));
        let handle = PersistentSinkHandle(Arc::clone(&metered));
        // Arc<MeteredSink<RollingFileSink>>: AuditSink via the blanket impl in audit_sink.rs.
        Self::with_sinks(vec![Box::new(metered)], Some(handle))
    }

    /// Create a logger that fans every sanitized event out to `sinks` in
    /// addition to the in-memory buffer and optional stdout output.
    ///
    /// For external callers that need custom sinks without a persistent-sink metrics handle.
    pub fn with_sinks(
        sinks: Vec<Box<dyn AuditSink>>,
        persistent: Option<PersistentSinkHandle>,
    ) -> Self {
        const MAX_RECENT_EVENTS: usize = 512;

        let (sender, receiver) = unbounded::<AuditMessage>();
        let recent_events = Arc::new(Mutex::new(VecDeque::new()));
        let stdout_enabled = env::var("AUDIT_LOG_STDOUT").is_ok();
        let recent_events_for_worker = Arc::clone(&recent_events);

        thread::spawn(move || {
            while let Ok(message) = receiver.recv() {
                let should_buffer_event = !matches!(message, AuditMessage::Event(_));
                let event = match message {
                    AuditMessage::Event(event) => Some(event),
                    AuditMessage::StateUpdate { previous, current } => {
                        match build_state_update_event(previous.as_deref(), current.as_ref()) {
                            Ok(event) => event,
                            Err(error) => {
                                eprintln!(
                                    "[AUDIT][ERROR] Failed to build state update event: {}",
                                    error
                                );
                                None
                            }
                        }
                    }
                };

                let Some(event) = event else {
                    continue;
                };

                let sanitized = sanitize_audit_event(event);
                if should_buffer_event {
                    push_recent_event(
                        &recent_events_for_worker,
                        sanitized.clone(),
                        MAX_RECENT_EVENTS,
                    );
                }

                // Fan out to persistent sinks (RollingFileSink, WebhookSink, …).
                for sink in &sinks {
                    if let Err(e) = sink.write(&sanitized) {
                        eprintln!("[AUDIT][ERROR] Sink '{}' failed: {}", sink.name(), e);
                    }
                }

                if stdout_enabled {
                    if let Ok(json) = serde_json::to_string(&sanitized) {
                        println!("[AUDIT] {}", json);
                    }
                }
            }
        });

        Self {
            sender,
            recent_events,
            persistent,
        }
    }

    /// Returns `(events_written, bytes_written)` from the persistent sink, or `None` if no
    /// persistent sink is configured.
    pub fn persistent_metrics(&self) -> Option<(u64, u64)> {
        self.persistent
            .as_ref()
            .map(|h| (h.events_written(), h.bytes_written()))
    }

    pub fn log(&self, event: AuditEvent) {
        const MAX_RECENT_EVENTS: usize = 512;

        let sanitized = sanitize_audit_event(event);
        push_recent_event(&self.recent_events, sanitized.clone(), MAX_RECENT_EVENTS);

        if let Err(e) = self.sender.send(AuditMessage::Event(sanitized)) {
            eprintln!("[AUDIT][ERROR] Failed to send audit event: {}", e);
        }
    }

    pub fn log_state_update(
        &self,
        previous: Option<Arc<SemanticState>>,
        current: Arc<SemanticState>,
    ) {
        if let Err(error) = self
            .sender
            .send(AuditMessage::StateUpdate { previous, current })
        {
            eprintln!(
                "[AUDIT][ERROR] Failed to send state update event: {}",
                error
            );
        }
    }

    pub fn recent_events(&self) -> Vec<AuditEvent> {
        self.recent_events
            .lock()
            .map(|events| events.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn clear_recent_events(&self) {
        if let Ok(mut guard) = self.recent_events.lock() {
            guard.clear();
        }
    }
}

fn push_recent_event(
    recent_events: &Arc<Mutex<VecDeque<AuditEvent>>>,
    event: AuditEvent,
    max_recent_events: usize,
) {
    if let Ok(mut guard) = recent_events.lock() {
        guard.push_back(event);
        while guard.len() > max_recent_events {
            guard.pop_front();
        }
    }
}

fn sanitize_audit_event(event: AuditEvent) -> AuditEvent {
    let r = privacy::global();
    match event {
        AuditEvent::ToolCall {
            tool_name,
            args,
            timestamp,
        } => AuditEvent::ToolCall {
            tool_name,
            args: r.redact_json_tool_args(&args),
            timestamp,
        },
        AuditEvent::StateSnapshot {
            state_hash,
            page_instance_id,
            timestamp,
            payload,
        } => AuditEvent::StateSnapshot {
            state_hash,
            page_instance_id,
            timestamp,
            payload: r.redact_json(&payload),
        },
        AuditEvent::StatePatch {
            state_hash,
            page_instance_id,
            timestamp,
            patch,
        } => AuditEvent::StatePatch {
            state_hash,
            page_instance_id,
            timestamp,
            patch: r.redact_json(&patch),
        },
        // Plugin-supplied strings (error_message, reason) may contain PII from
        // page content (URLs with tokens, emails, etc.).  Truncate them to a safe
        // maximum length and redact any recognised sensitive patterns.
        AuditEvent::PluginStateTransform {
            plugin_id,
            success,
            error_message,
            timestamp,
        } => AuditEvent::PluginStateTransform {
            plugin_id,
            success,
            error_message: error_message
                .as_deref()
                .map(|s| r.redact_text(s))
                .map(|s| truncate_plugin_string(&s)),
            timestamp,
        },
        AuditEvent::PluginPolicyDecision {
            plugin_id,
            allowed,
            reason,
            timestamp,
        } => AuditEvent::PluginPolicyDecision {
            plugin_id,
            allowed,
            reason: reason
                .as_deref()
                .map(|s| r.redact_text(s))
                .map(|s| truncate_plugin_string(&s)),
            timestamp,
        },
        other => other,
    }
}

/// Truncate a plugin-supplied string to at most `MAX_PLUGIN_STRING_LEN` characters.
/// Appends `" [truncated]"` when truncation occurs.
const MAX_PLUGIN_STRING_LEN: usize = 200;

fn truncate_plugin_string(s: &str) -> String {
    if s.len() <= MAX_PLUGIN_STRING_LEN {
        s.to_string()
    } else {
        format!("{} [truncated]", &s[..MAX_PLUGIN_STRING_LEN])
    }
}

fn build_state_update_event(
    previous: Option<&SemanticState>,
    current: &SemanticState,
) -> anyhow::Result<Option<AuditEvent>> {
    let timestamp = epoch_millis_u64();

    let Some(previous) = previous else {
        return Ok(Some(AuditEvent::StateSnapshot {
            state_hash: current.state_hash().to_string(),
            page_instance_id: current.page_instance_id().to_string(),
            timestamp,
            payload: serde_json::to_value(current.root())?,
        }));
    };

    let Some(delta) = current.build_delta(previous)? else {
        return Ok(None);
    };

    Ok(Some(AuditEvent::StatePatch {
        state_hash: current.state_hash().to_string(),
        page_instance_id: current.page_instance_id().to_string(),
        timestamp,
        patch: serde_json::to_value(&delta.patch)?,
    }))
}

fn epoch_millis_u64() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::{
        sanitize_audit_event, truncate_plugin_string, AuditEvent, AuditLogger,
        MAX_PLUGIN_STRING_LEN,
    };
    use crate::privacy::PiiRedactor;
    use serde_json::json;

    #[test]
    fn redact_sensitive_text_masks_card_numbers_up_to_nineteen_digits() {
        let r = PiiRedactor::new();
        let cases = [
            (
                "Card 4111-1111-1111-1111 for alice@example.com",
                "Card ****-****-****-XXXX for ***",
            ),
            (
                "Card 4000 1234 5678 9012 345 for alice@example.com",
                "Card ****-****-****-XXXX for ***",
            ),
            (
                "Card 4000123456789012345 for alice@example.com",
                "Card ****-****-****-XXXX for ***",
            ),
        ];

        for (input, expected) in cases {
            assert_eq!(r.redact_text(input), expected);
        }
    }

    #[test]
    fn log_buffers_direct_events_before_worker_drain() {
        let logger = AuditLogger::new();
        logger.clear_recent_events();

        logger.log(AuditEvent::ToolCall {
            tool_name: "act".to_string(),
            args: json!({ "value": "sensitive@example.com" }),
            timestamp: 1,
        });

        let events = logger.recent_events();
        assert_eq!(
            events.len(),
            1,
            "direct audit events must be buffered eagerly"
        );
        assert!(matches!(
            &events[0],
            AuditEvent::ToolCall { tool_name, .. } if tool_name == "act"
        ));
    }

    #[test]
    fn truncate_plugin_string_keeps_short_strings_unchanged() {
        let short = "short error";
        assert_eq!(truncate_plugin_string(short), short);
    }

    #[test]
    fn truncate_plugin_string_truncates_long_strings() {
        let long_str = "x".repeat(MAX_PLUGIN_STRING_LEN + 50);
        let result = truncate_plugin_string(&long_str);
        assert!(result.ends_with(" [truncated]"));
        assert!(result.len() <= MAX_PLUGIN_STRING_LEN + " [truncated]".len());
    }

    #[test]
    fn sanitize_plugin_state_transform_truncates_long_error_message() {
        let long_msg = "e".repeat(300);
        let event = AuditEvent::PluginStateTransform {
            plugin_id: "test-plugin".to_string(),
            success: false,
            error_message: Some(long_msg),
            timestamp: 0,
        };
        let sanitized = sanitize_audit_event(event);
        if let AuditEvent::PluginStateTransform { error_message, .. } = sanitized {
            let msg = error_message.expect("error_message should be present");
            assert!(
                msg.ends_with(" [truncated]"),
                "long error_message must be truncated"
            );
            assert!(
                msg.len() <= MAX_PLUGIN_STRING_LEN + " [truncated]".len(),
                "truncated message must not exceed max length + suffix"
            );
        } else {
            panic!("Expected PluginStateTransform event");
        }
    }

    #[test]
    fn sanitize_plugin_policy_decision_truncates_long_reason() {
        let long_reason = "r".repeat(300);
        let event = AuditEvent::PluginPolicyDecision {
            plugin_id: "test-plugin".to_string(),
            allowed: false,
            reason: Some(long_reason),
            timestamp: 0,
        };
        let sanitized = sanitize_audit_event(event);
        if let AuditEvent::PluginPolicyDecision { reason, .. } = sanitized {
            let r = reason.expect("reason should be present");
            assert!(r.ends_with(" [truncated]"), "long reason must be truncated");
            assert!(
                r.len() <= MAX_PLUGIN_STRING_LEN + " [truncated]".len(),
                "truncated reason must not exceed max length + suffix"
            );
        } else {
            panic!("Expected PluginPolicyDecision event");
        }
    }

    #[test]
    fn sanitize_plugin_state_transform_redacts_email_in_error_message() {
        let event = AuditEvent::PluginStateTransform {
            plugin_id: "test-plugin".to_string(),
            success: false,
            error_message: Some("failed for user@example.com".to_string()),
            timestamp: 0,
        };
        let sanitized = sanitize_audit_event(event);
        if let AuditEvent::PluginStateTransform { error_message, .. } = sanitized {
            let msg = error_message.expect("error_message should be present");
            assert!(!msg.contains("user@example.com"), "email must be redacted");
            assert!(msg.contains("***"), "email should be replaced with ***");
        } else {
            panic!("Expected PluginStateTransform event");
        }
    }

    #[test]
    fn sanitize_plugin_policy_decision_redacts_email_in_reason() {
        let event = AuditEvent::PluginPolicyDecision {
            plugin_id: "test-plugin".to_string(),
            allowed: false,
            reason: Some("blocked because of admin@corp.example.com".to_string()),
            timestamp: 0,
        };
        let sanitized = sanitize_audit_event(event);
        if let AuditEvent::PluginPolicyDecision { reason, .. } = sanitized {
            let r = reason.expect("reason should be present");
            assert!(
                !r.contains("admin@corp.example.com"),
                "email must be redacted"
            );
            assert!(r.contains("***"), "email should be replaced with ***");
        } else {
            panic!("Expected PluginPolicyDecision event");
        }
    }

    #[test]
    fn sanitize_plugin_events_preserve_none_fields() {
        let state_event = AuditEvent::PluginStateTransform {
            plugin_id: "p".to_string(),
            success: true,
            error_message: None,
            timestamp: 0,
        };
        let policy_event = AuditEvent::PluginPolicyDecision {
            plugin_id: "p".to_string(),
            allowed: true,
            reason: None,
            timestamp: 0,
        };

        if let AuditEvent::PluginStateTransform { error_message, .. } =
            sanitize_audit_event(state_event)
        {
            assert!(error_message.is_none());
        }
        if let AuditEvent::PluginPolicyDecision { reason, .. } = sanitize_audit_event(policy_event)
        {
            assert!(reason.is_none());
        }
    }
}
