use crate::audit::AuditEvent;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReplayError {
    #[error(
        "STATE_PATCH at timestamp {timestamp} (hash {state_hash}) has no preceding STATE_SNAPSHOT"
    )]
    NoBaseSnapshot { state_hash: String, timestamp: u64 },
    #[error(
        "patch application failed for state_hash {state_hash} at timestamp {timestamp}: {reason}"
    )]
    PatchFailed {
        state_hash: String,
        timestamp: u64,
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateChainEntry {
    pub state_hash: String,
    pub timestamp: u64,
    pub kind: StateEntryKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateEntryKind {
    Snapshot,
    Patch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallEntry {
    pub tool_name: String,
    pub timestamp: u64,
    pub has_redacted_args: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDecisionEntry {
    pub rule_id: String,
    pub action: String,
    pub decision: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HitlEventEntry {
    pub event_type: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayReport {
    pub total_events: usize,
    pub has_redacted_content: bool,
    pub state_chain: Vec<StateChainEntry>,
    pub tool_calls: Vec<ToolCallEntry>,
    pub policy_decisions: Vec<PolicyDecisionEntry>,
    pub hitl_events: Vec<HitlEventEntry>,
}

/// Process a slice of `AuditEvent`s and produce a `ReplayReport`.
///
/// STATE_PATCH events are applied against the current snapshot using RFC 6902 JSON Patch.
/// Returns `Err` on the first invalid patch (no prior snapshot, or patch application failure).
///
/// **Note:** on error, all accumulated state chain / tool calls / decisions up to the
/// failing event are discarded. Callers that need partial output should split the log
/// at the error boundary and replay each segment independently.
pub fn replay_events(events: &[AuditEvent]) -> Result<ReplayReport, ReplayError> {
    let mut state_chain: Vec<StateChainEntry> = Vec::new();
    let mut tool_calls: Vec<ToolCallEntry> = Vec::new();
    let mut policy_decisions: Vec<PolicyDecisionEntry> = Vec::new();
    let mut hitl_events: Vec<HitlEventEntry> = Vec::new();
    let mut has_redacted_content = false;

    let mut current_snapshot: Option<serde_json::Value> = None;

    for event in events {
        match event {
            AuditEvent::StateSnapshot {
                state_hash,
                timestamp,
                payload,
                ..
            } => {
                current_snapshot = Some(payload.clone());
                state_chain.push(StateChainEntry {
                    state_hash: state_hash.clone(),
                    timestamp: *timestamp,
                    kind: StateEntryKind::Snapshot,
                });
                if json_contains_redaction(payload) {
                    has_redacted_content = true;
                }
            }

            AuditEvent::StatePatch {
                state_hash,
                timestamp,
                patch,
                ..
            } => {
                let base =
                    current_snapshot
                        .as_mut()
                        .ok_or_else(|| ReplayError::NoBaseSnapshot {
                            state_hash: state_hash.clone(),
                            timestamp: *timestamp,
                        })?;

                let ops: json_patch::Patch =
                    serde_json::from_value(patch.clone()).map_err(|e| {
                        ReplayError::PatchFailed {
                            state_hash: state_hash.clone(),
                            timestamp: *timestamp,
                            reason: format!("invalid patch JSON: {e}"),
                        }
                    })?;

                json_patch::patch(base, &ops).map_err(|e| ReplayError::PatchFailed {
                    state_hash: state_hash.clone(),
                    timestamp: *timestamp,
                    reason: e.to_string(),
                })?;

                if json_contains_redaction(patch) {
                    has_redacted_content = true;
                }

                state_chain.push(StateChainEntry {
                    state_hash: state_hash.clone(),
                    timestamp: *timestamp,
                    kind: StateEntryKind::Patch,
                });
            }

            AuditEvent::ToolCall {
                tool_name,
                args,
                timestamp,
            } => {
                let has_redacted_args = json_contains_redaction(args);
                if has_redacted_args {
                    has_redacted_content = true;
                }
                tool_calls.push(ToolCallEntry {
                    tool_name: tool_name.clone(),
                    timestamp: *timestamp,
                    has_redacted_args,
                });
            }

            AuditEvent::PolicyDecision {
                rule_id,
                action,
                decision,
                timestamp,
            } => {
                policy_decisions.push(PolicyDecisionEntry {
                    rule_id: rule_id.clone(),
                    action: action.clone(),
                    decision: decision.clone(),
                    timestamp: *timestamp,
                });
            }

            AuditEvent::HitlEvent {
                event_type,
                timestamp,
                ..
            } => {
                hitl_events.push(HitlEventEntry {
                    event_type: event_type.clone(),
                    timestamp: *timestamp,
                });
            }

            // Visual captures and plugin events don't contribute to the state chain
            // or decision records but are counted in total_events.
            AuditEvent::VisualCapture { .. }
            | AuditEvent::PluginStateTransform { .. }
            | AuditEvent::PluginPolicyDecision { .. } => {}
        }
    }

    Ok(ReplayReport {
        total_events: events.len(),
        has_redacted_content,
        state_chain,
        tool_calls,
        policy_decisions,
        hitl_events,
    })
}

/// Return a Markdown-formatted report string from a `ReplayReport`.
pub fn report_to_markdown(report: &ReplayReport) -> String {
    let mut out = String::new();
    out.push_str("# Audit Replay Report\n\n");
    out.push_str(&format!("**Total events:** {}\n", report.total_events));
    out.push_str(&format!(
        "**Redacted content detected:** {}\n\n",
        report.has_redacted_content
    ));

    out.push_str("## State Chain\n\n");
    if report.state_chain.is_empty() {
        out.push_str("_(no state events)_\n");
    } else {
        out.push_str("| # | Kind | Hash | Timestamp (ms) |\n");
        out.push_str("|---|------|------|----------------|\n");
        for (i, entry) in report.state_chain.iter().enumerate() {
            let kind = match &entry.kind {
                StateEntryKind::Snapshot => "SNAPSHOT",
                StateEntryKind::Patch => "PATCH",
            };
            out.push_str(&format!(
                "| {} | {} | `{}` | {} |\n",
                i + 1,
                kind,
                escape_md(&entry.state_hash),
                entry.timestamp
            ));
        }
    }
    out.push('\n');

    out.push_str("## Tool Calls\n\n");
    if report.tool_calls.is_empty() {
        out.push_str("_(no tool calls)_\n");
    } else {
        out.push_str("| Tool | Timestamp (ms) | Redacted Args |\n");
        out.push_str("|------|----------------|---------------|\n");
        for tc in &report.tool_calls {
            out.push_str(&format!(
                "| `{}` | {} | {} |\n",
                escape_md(&tc.tool_name),
                tc.timestamp,
                tc.has_redacted_args
            ));
        }
    }
    out.push('\n');

    out.push_str("## Policy Decisions\n\n");
    if report.policy_decisions.is_empty() {
        out.push_str("_(no policy decisions)_\n");
    } else {
        out.push_str("| Rule | Action | Decision | Timestamp (ms) |\n");
        out.push_str("|------|--------|----------|----------------|\n");
        for pd in &report.policy_decisions {
            out.push_str(&format!(
                "| `{}` | {} | **{}** | {} |\n",
                escape_md(&pd.rule_id),
                escape_md(&pd.action),
                escape_md(&pd.decision),
                pd.timestamp
            ));
        }
    }
    out.push('\n');

    out.push_str("## HITL Events\n\n");
    if report.hitl_events.is_empty() {
        out.push_str("_(no HITL events)_\n");
    } else {
        out.push_str("| Event Type | Timestamp (ms) |\n");
        out.push_str("|------------|----------------|\n");
        for he in &report.hitl_events {
            out.push_str(&format!(
                "| `{}` | {} |\n",
                escape_md(&he.event_type),
                he.timestamp
            ));
        }
    }

    out
}

fn escape_md(s: &str) -> String {
    s.replace('`', "\\`").replace('|', "\\|").replace('\n', " ")
}

fn json_contains_redaction(value: &serde_json::Value) -> bool {
    match value {
        // "***" covers email/generic redaction; "****-****-****-" covers card numbers.
        serde_json::Value::String(s) => s.contains("***") || s.contains("****-****-****-"),
        serde_json::Value::Array(arr) => arr.iter().any(json_contains_redaction),
        serde_json::Value::Object(map) => map.values().any(json_contains_redaction),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_contains_redaction_detects_star_marker() {
        assert!(json_contains_redaction(&json!("***")));
        assert!(json_contains_redaction(&json!({"v": "***"})));
        assert!(json_contains_redaction(&json!(["ok", "***"])));
        assert!(!json_contains_redaction(&json!("clean")));
        assert!(!json_contains_redaction(&json!(42)));
    }

    #[test]
    fn json_contains_redaction_detects_card_marker() {
        assert!(json_contains_redaction(&json!("Card: ****-****-****-XXXX")));
        assert!(json_contains_redaction(
            &json!({"card": "****-****-****-XXXX"})
        ));
        assert!(!json_contains_redaction(&json!("1234-5678-9012-3456")));
    }

    #[test]
    fn report_to_markdown_empty_report_renders_empty_sections() {
        let report = ReplayReport {
            total_events: 0,
            has_redacted_content: false,
            state_chain: vec![],
            tool_calls: vec![],
            policy_decisions: vec![],
            hitl_events: vec![],
        };
        let md = report_to_markdown(&report);
        assert!(md.contains("_(no state events)_"));
        assert!(md.contains("_(no tool calls)_"));
        assert!(md.contains("_(no policy decisions)_"));
        assert!(md.contains("_(no HITL events)_"));
    }

    #[test]
    fn escape_md_sanitizes_injection_characters() {
        assert_eq!(escape_md("a|b"), "a\\|b");
        assert_eq!(escape_md("a`b"), "a\\`b");
        assert_eq!(escape_md("a\nb"), "a b");
        assert_eq!(escape_md("safe"), "safe");
    }

    #[test]
    fn empty_event_list_produces_empty_report() {
        let report = replay_events(&[]).expect("empty replay should succeed");
        assert_eq!(report.total_events, 0);
        assert!(!report.has_redacted_content);
        assert!(report.state_chain.is_empty());
    }

    #[test]
    fn snapshot_only_sets_chain_entry() {
        let events = vec![AuditEvent::StateSnapshot {
            state_hash: "sha256:abc".to_string(),
            page_instance_id: "p-1".to_string(),
            timestamp: 500,
            payload: json!({"role": "document"}),
        }];
        let report = replay_events(&events).unwrap();
        assert_eq!(report.state_chain.len(), 1);
        assert!(matches!(
            report.state_chain[0].kind,
            StateEntryKind::Snapshot
        ));
    }

    #[test]
    fn report_to_markdown_contains_key_sections() {
        let report = ReplayReport {
            total_events: 3,
            has_redacted_content: true,
            state_chain: vec![StateChainEntry {
                state_hash: "sha256:abc".to_string(),
                timestamp: 1000,
                kind: StateEntryKind::Snapshot,
            }],
            tool_calls: vec![ToolCallEntry {
                tool_name: "act".to_string(),
                timestamp: 2000,
                has_redacted_args: true,
            }],
            policy_decisions: vec![PolicyDecisionEntry {
                rule_id: "R-1".to_string(),
                action: "redact".to_string(),
                decision: "allow".to_string(),
                timestamp: 3000,
            }],
            hitl_events: vec![],
        };

        let md = report_to_markdown(&report);
        assert!(md.contains("# Audit Replay Report"));
        assert!(md.contains("Total events:** 3"));
        assert!(md.contains("SNAPSHOT"));
        assert!(md.contains("sha256:abc"));
        assert!(md.contains("R-1"));
    }
}
