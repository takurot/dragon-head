use crate::sre::SemanticState;
use crossbeam_channel::{unbounded, Sender};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    env,
    sync::{Arc, Mutex, OnceLock},
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
}

#[derive(Clone)]
pub struct AuditLogger {
    sender: Sender<AuditMessage>,
    recent_events: Arc<Mutex<VecDeque<AuditEvent>>>,
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
    pub fn new() -> Self {
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

                if !stdout_enabled {
                    continue;
                }
                if let Ok(json) = serde_json::to_string(&sanitized) {
                    println!("[AUDIT] {}", json);
                }
            }
        });

        Self {
            sender,
            recent_events,
        }
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
    match event {
        AuditEvent::ToolCall {
            tool_name,
            args,
            timestamp,
        } => AuditEvent::ToolCall {
            tool_name,
            args: redact_json_value(&args, None, true),
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
            payload: redact_json_value(&payload, None, false),
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
            patch: redact_json_value(&patch, None, false),
        },
        // Plugin audit events do not contain PII by design.
        other @ AuditEvent::PluginStateTransform { .. }
        | other @ AuditEvent::PluginPolicyDecision { .. } => other,
        other => other,
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

fn redact_json_value(
    value: &serde_json::Value,
    key_hint: Option<&str>,
    mask_tool_value_field: bool,
) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut redacted = serde_json::Map::with_capacity(map.len());
            for (key, child) in map {
                let key_lower = key.to_ascii_lowercase();
                let masked_child = if is_sensitive_key(&key_lower)
                    || (mask_tool_value_field && should_mask_tool_argument_key(&key_lower))
                {
                    serde_json::Value::String("***".to_string())
                } else {
                    redact_json_value(child, Some(key_lower.as_str()), mask_tool_value_field)
                };
                redacted.insert(key.clone(), masked_child);
            }
            serde_json::Value::Object(redacted)
        }
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(|item| redact_json_value(item, key_hint, mask_tool_value_field))
                .collect(),
        ),
        serde_json::Value::String(text) => {
            if key_hint.is_some_and(is_sensitive_key) {
                serde_json::Value::String("***".to_string())
            } else {
                serde_json::Value::String(redact_sensitive_text(text))
            }
        }
        _ => value.clone(),
    }
}

fn should_mask_tool_argument_key(key: &str) -> bool {
    key == "value" || key == "text" || key.ends_with("_text")
}

fn is_sensitive_key(key: &str) -> bool {
    matches!(
        key,
        "password"
            | "passwd"
            | "email"
            | "token"
            | "secret"
            | "authorization"
            | "auth"
            | "card"
            | "credit_card"
            | "cc"
            | "cvv"
            | "cvc"
    ) || key.contains("password")
        || key.contains("email")
        || key.contains("token")
        || key.contains("secret")
        || key.contains("card")
}

fn redact_sensitive_text(text: &str) -> String {
    static CC_RE: OnceLock<Regex> = OnceLock::new();
    static EMAIL_RE: OnceLock<Regex> = OnceLock::new();

    let cc_re =
        CC_RE.get_or_init(|| Regex::new(r"\b\d(?:[ -]?\d){12,18}\b").expect("Invalid CC regex"));
    let email_re = EMAIL_RE.get_or_init(|| {
        Regex::new(r"(?i)\b[a-z0-9._%+\-]+@[a-z0-9.\-]+\.[a-z]{2,}\b").expect("Invalid email regex")
    });

    let cc_redacted = cc_re.replace_all(text, "****-****-****-XXXX");
    email_re.replace_all(&cc_redacted, "***").into_owned()
}

#[cfg(test)]
mod tests {
    use super::{redact_sensitive_text, AuditEvent, AuditLogger};
    use serde_json::json;

    #[test]
    fn redact_sensitive_text_masks_card_numbers_up_to_nineteen_digits() {
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
            assert_eq!(redact_sensitive_text(input), expected);
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
}
