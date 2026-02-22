use crossbeam_channel::{unbounded, Sender};
use serde::{Deserialize, Serialize};
use std::thread;

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
}

impl Default for AuditLogger {
    fn default() -> Self {
        Self::new()
    }
}

impl AuditLogger {
    pub fn new() -> Self {
        let (sender, receiver) = unbounded::<AuditEvent>();

        thread::spawn(move || {
            while let Ok(event) = receiver.recv() {
                if let Ok(json) = serde_json::to_string(&event) {
                    // For now, write audit events to stdout prefixed with [AUDIT]
                    println!("[AUDIT] {}", json);
                }
            }
        });

        Self { sender }
    }

    pub fn log(&self, event: AuditEvent) {
        let _ = self.sender.send(event);
    }
}
