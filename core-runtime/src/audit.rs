use crossbeam_channel::{unbounded, Sender};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, OnceLock},
    thread,
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
}

#[derive(Clone)]
pub struct AuditLogger {
    sender: Sender<AuditEvent>,
    recent_events: Arc<Mutex<VecDeque<AuditEvent>>>,
}

impl Default for AuditLogger {
    fn default() -> Self {
        Self::new()
    }
}

impl AuditLogger {
    pub fn new() -> Self {
        let (sender, receiver) = unbounded::<AuditEvent>();
        let recent_events = Arc::new(Mutex::new(VecDeque::new()));

        thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                if let Ok(json) = serde_json::to_string(&event) {
                    // For now, write audit events to stdout prefixed with [AUDIT]
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
        if let Ok(mut guard) = self.recent_events.lock() {
            guard.push_back(sanitized.clone());
            while guard.len() > MAX_RECENT_EVENTS {
                guard.pop_front();
            }
        }

        if let Err(e) = self.sender.send(sanitized) {
            eprintln!("[AUDIT][ERROR] Failed to send audit event: {}", e);
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
        other => other,
    }
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
    use super::redact_sensitive_text;

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
}
