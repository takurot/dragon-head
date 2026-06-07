//! Relays approval requests to a chat tool and posts back the resolution.
//!
//! [`ChatNotifier`] is the trait boundary the rest of the bridge depends on;
//! [`SlackNotifier`] is the reference Slack Block Kit implementation. A Teams
//! notifier is a documented extension point — implement the same trait against
//! the Teams Adaptive Cards API.

use anyhow::{Context, Result};
use core_runtime::{OutcomeProjection, RiskLevel};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::lock::Decision;

/// Everything the notifier needs to render an approval prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct ApprovalNotification {
    pub id: Uuid,
    pub rule_id: String,
    pub action: String,
    pub outcome: Option<OutcomeProjection>,
    /// Set-of-Mark screenshot of the page at the moment the request was
    /// raised, if available. Rendered as an inline base64 data URL in the
    /// reference implementation — production deployments should upload via
    /// `files.upload` and reference the returned URL instead (see
    /// `docs/hitl-slack-bridge.md`).
    pub som_image_png: Option<Vec<u8>>,
}

/// Sends approval prompts to a chat tool and updates them once resolved.
///
/// Implementors must be safe to share across the polling loop and the HTTP
/// handler threads.
pub trait ChatNotifier: Send + Sync {
    /// Post a new approval prompt. Returns an opaque token (e.g. a Slack
    /// `channel:ts` pair) that [`respond`](Self::respond) uses to update the
    /// original message.
    fn notify(&self, notification: &ApprovalNotification) -> Result<String>;

    /// Replace the original prompt with the final resolution.
    fn respond(&self, token: &str, decision: Decision, decided_by: &str) -> Result<()>;
}

fn risk_label(level: &RiskLevel) -> &'static str {
    match level {
        RiskLevel::Low => "Low",
        RiskLevel::Medium => "Medium",
        RiskLevel::High => "High",
        RiskLevel::Critical => "Critical",
    }
}

fn outcome_field_text(outcome: &Option<OutcomeProjection>) -> String {
    match outcome {
        Some(projection) => {
            let amount = projection
                .projected_amount
                .map(|amount| format!("${amount:.2}"))
                .unwrap_or_else(|| "(none detected)".to_string());
            format!(
                "*Outcome Projection*\nProjected amount: {amount}\nRisk level: {}",
                risk_label(&projection.risk_level)
            )
        }
        None => "*Outcome Projection*\n(not available)".to_string(),
    }
}

/// Builds the Slack Block Kit message body for an approval prompt.
///
/// Exposed so tests (and `MockNotifier`) can assert on the exact JSON shape
/// without going over the network.
pub fn build_approval_blocks(notification: &ApprovalNotification) -> Value {
    let mut blocks = vec![
        json!({
            "type": "section",
            "text": {
                "type": "mrkdwn",
                "text": format!(
                    "*Human approval requested*\n*Reason:* rule `{}` requires approval\n*Action intent:* `{}`",
                    notification.rule_id, notification.action,
                ),
            },
        }),
        json!({
            "type": "section",
            "text": {
                "type": "mrkdwn",
                "text": outcome_field_text(&notification.outcome),
            },
        }),
    ];

    if let Some(image_png) = &notification.som_image_png {
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(image_png);
        blocks.push(json!({
            "type": "image",
            "image_url": format!("data:image/png;base64,{encoded}"),
            "alt_text": "Set-of-Mark capture of the page at the time of the request",
        }));
    }

    blocks.push(json!({
        "type": "actions",
        "elements": [
            {
                "type": "button",
                "text": { "type": "plain_text", "text": "Approve" },
                "style": "primary",
                "action_id": "approve",
                "value": notification.id.to_string(),
            },
            {
                "type": "button",
                "text": { "type": "plain_text", "text": "Reject" },
                "style": "danger",
                "action_id": "reject",
                "value": notification.id.to_string(),
            },
        ],
    }));

    json!({ "blocks": blocks })
}

/// Builds the Slack Block Kit message body for a resolved prompt — replaces
/// the interactive buttons with a static resolution line.
pub fn build_resolution_blocks(decision: Decision, decided_by: &str) -> Value {
    let verb = match decision {
        Decision::Approved => "approved",
        Decision::Rejected => "rejected",
    };
    json!({
        "blocks": [
            {
                "type": "section",
                "text": {
                    "type": "mrkdwn",
                    "text": format!("*Resolved:* {verb} by *{decided_by}*"),
                },
            },
        ],
    })
}

/// Reference Slack implementation of [`ChatNotifier`].
///
/// Posts via `chat.postMessage` and updates via `chat.update`, both of which
/// accept the same Block Kit body shape — the resolution token is the
/// `"{channel}:{ts}"` pair Slack returns from `postMessage`.
pub struct SlackNotifier {
    client: reqwest::blocking::Client,
    bot_token: String,
    channel: String,
}

impl SlackNotifier {
    pub fn new(bot_token: impl Into<String>, channel: impl Into<String>) -> Self {
        Self {
            client: reqwest::blocking::Client::new(),
            bot_token: bot_token.into(),
            channel: channel.into(),
        }
    }

    fn post(&self, method: &str, body: &Value) -> Result<Value> {
        let response = self
            .client
            .post(format!("https://slack.com/api/{method}"))
            .bearer_auth(&self.bot_token)
            .json(body)
            .send()
            .with_context(|| format!("failed to call Slack API method '{method}'"))?;

        let payload: Value = response
            .json()
            .with_context(|| format!("failed to parse Slack API response for '{method}'"))?;

        if payload.get("ok").and_then(Value::as_bool) != Some(true) {
            anyhow::bail!(
                "Slack API method '{method}' returned an error: {}",
                payload
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            );
        }

        Ok(payload)
    }
}

impl ChatNotifier for SlackNotifier {
    fn notify(&self, notification: &ApprovalNotification) -> Result<String> {
        let mut body = build_approval_blocks(notification);
        body["channel"] = json!(self.channel);

        let payload = self.post("chat.postMessage", &body)?;
        let ts = payload
            .get("ts")
            .and_then(Value::as_str)
            .context("Slack chat.postMessage response missing 'ts'")?;

        Ok(format!("{}:{ts}", self.channel))
    }

    fn respond(&self, token: &str, decision: Decision, decided_by: &str) -> Result<()> {
        let (channel, ts) = token
            .split_once(':')
            .context("malformed Slack resolution token; expected 'channel:ts'")?;

        let mut body = build_resolution_blocks(decision, decided_by);
        body["channel"] = json!(channel);
        body["ts"] = json!(ts);

        self.post("chat.update", &body)?;
        Ok(())
    }
}

/// In-memory [`ChatNotifier`] for tests — records every call.
pub mod mock {
    use super::*;
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq)]
    pub enum Call {
        Notify(ApprovalNotification),
        Respond {
            token: String,
            decision: Decision,
            decided_by: String,
        },
    }

    #[derive(Default)]
    pub struct MockNotifier {
        calls: Mutex<Vec<Call>>,
    }

    impl MockNotifier {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn calls(&self) -> Vec<Call> {
            self.calls
                .lock()
                .expect("mock notifier mutex poisoned")
                .clone()
        }
    }

    impl ChatNotifier for MockNotifier {
        fn notify(&self, notification: &ApprovalNotification) -> Result<String> {
            let token = format!("mock-channel:{}", notification.id);
            self.calls
                .lock()
                .expect("mock notifier mutex poisoned")
                .push(Call::Notify(notification.clone()));
            Ok(token)
        }

        fn respond(&self, token: &str, decision: Decision, decided_by: &str) -> Result<()> {
            self.calls
                .lock()
                .expect("mock notifier mutex poisoned")
                .push(Call::Respond {
                    token: token.to_string(),
                    decision,
                    decided_by: decided_by.to_string(),
                });
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_notification() -> ApprovalNotification {
        ApprovalNotification {
            id: Uuid::new_v4(),
            rule_id: "approve-pay".to_string(),
            action: "click".to_string(),
            outcome: Some(OutcomeProjection {
                projected_amount: Some(900.5),
                risk_level: RiskLevel::High,
            }),
            som_image_png: Some(vec![1, 2, 3, 4]),
        }
    }

    #[test]
    fn approval_blocks_include_reason_action_and_outcome_text() {
        let notification = sample_notification();
        let body = build_approval_blocks(&notification);
        let rendered = body.to_string();

        assert!(rendered.contains("approve-pay"));
        assert!(rendered.contains("`click`"));
        assert!(rendered.contains("Projected amount: $900.50"));
        assert!(rendered.contains("Risk level: High"));
    }

    #[test]
    fn approval_blocks_wire_button_action_ids_and_values_to_the_request_id() {
        let notification = sample_notification();
        let body = build_approval_blocks(&notification);
        let blocks = body["blocks"].as_array().expect("blocks array");

        let actions = blocks
            .iter()
            .find(|block| block["type"] == "actions")
            .expect("an actions block must be present");
        let elements = actions["elements"].as_array().expect("elements array");

        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0]["action_id"], "approve");
        assert_eq!(elements[0]["value"], notification.id.to_string());
        assert_eq!(elements[1]["action_id"], "reject");
        assert_eq!(elements[1]["value"], notification.id.to_string());
    }

    #[test]
    fn approval_blocks_render_som_image_as_inline_data_url_when_present() {
        let notification = sample_notification();
        let body = build_approval_blocks(&notification);
        let blocks = body["blocks"].as_array().expect("blocks array");

        let image = blocks
            .iter()
            .find(|block| block["type"] == "image")
            .expect("an image block must be present when SoM data is attached");
        assert!(image["image_url"]
            .as_str()
            .expect("image_url string")
            .starts_with("data:image/png;base64,"));
    }

    #[test]
    fn approval_blocks_omit_image_block_when_som_capture_is_absent() {
        let mut notification = sample_notification();
        notification.som_image_png = None;
        let body = build_approval_blocks(&notification);
        let blocks = body["blocks"].as_array().expect("blocks array");

        assert!(!blocks.iter().any(|block| block["type"] == "image"));
    }

    #[test]
    fn approval_blocks_render_placeholder_when_outcome_projection_missing() {
        let mut notification = sample_notification();
        notification.outcome = None;
        let body = build_approval_blocks(&notification);

        assert!(body
            .to_string()
            .contains("Outcome Projection*\\n(not available)"));
    }

    #[test]
    fn resolution_blocks_state_who_decided_and_what() {
        let approved = build_resolution_blocks(Decision::Approved, "alice");
        let rejected = build_resolution_blocks(Decision::Rejected, "bob");

        assert!(approved.to_string().contains("approved by *alice*"));
        assert!(rejected.to_string().contains("rejected by *bob*"));
    }
}
