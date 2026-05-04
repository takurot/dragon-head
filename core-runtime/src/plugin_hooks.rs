/// Plugin hook integration for core-runtime.
///
/// This module defines the traits and types that let external plugins intercept
/// two key points in the core-runtime pipeline:
///
/// * **State hooks** (`StatePlugin::on_state`): called after `normalize_dom`
///   produces a `SemanticState`, before the state is delivered externally.
///   Failures are *non-fatal* — logged to audit, original state is preserved.
///
/// * **Policy hooks** (`PolicyPlugin::before_act`): called before an action is
///   executed, after the built-in `PolicyEngine` allows it.  If any plugin
///   returns `{"allow": false}`, the action is blocked (fail-closed).
///   Failures (plugin trap / invalid JSON) are also treated as a block.
///
/// Both hook types record their decisions in the `AuditLogger` so they are
/// fully observable without leaking PII.
use crate::audit::AuditEvent;
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// A plugin that can observe and transform serialized semantic state.
///
/// The `on_state` method receives the current semantic state as a JSON string
/// and must return either a (possibly modified) JSON string or an error
/// description.  On error the original state is preserved and the failure is
/// recorded in the audit log (non-fatal).
pub trait StatePlugin: Send + Sync {
    /// Unique identifier for this plugin, used in audit records.
    fn plugin_id(&self) -> &str;

    /// Transform the semantic state JSON.  Return the (possibly unchanged)
    /// JSON on success, or an error message string on failure.
    fn on_state(&self, state_json: &str) -> Result<String, String>;
}

/// A plugin that can observe or veto an intended action before it is executed.
///
/// The `before_act` method receives a JSON object describing the intent and
/// must return a JSON object containing at least `{"allow": <bool>}`.
/// Returning `{"allow": false}` (or any error) causes the action to be
/// blocked (fail-closed).
pub trait PolicyPlugin: Send + Sync {
    /// Unique identifier for this plugin, used in audit records.
    fn plugin_id(&self) -> &str;

    /// Evaluate the intent JSON.  Return a JSON response containing at least
    /// `{"allow": true|false}`, or an error message string on failure
    /// (treated as block).
    fn before_act(&self, intent_json: &str) -> Result<String, String>;
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Holds the active plugin hooks for a browser client.
///
/// Both `state_plugins` and `policy_plugins` are stored as trait objects so
/// that callers can use any implementation (real Wasm-backed plugins via
/// `plugin-host`, mock implementations in tests, etc.).
#[derive(Default)]
pub struct PluginHookConfig {
    /// Plugins called after each `normalize_dom` state capture.
    pub state_plugins: Vec<Box<dyn StatePlugin>>,
    /// Plugins called before executing each action (after built-in policy).
    pub policy_plugins: Vec<Box<dyn PolicyPlugin>>,
}

// ---------------------------------------------------------------------------
// Outcome types
// ---------------------------------------------------------------------------

/// The combined result of running all policy plugins.
#[derive(Debug, PartialEq, Eq)]
pub enum PolicyHookOutcome {
    /// All plugins allowed the action (or there are no plugins).
    Allow,
    /// At least one plugin blocked the action.
    Block {
        plugin_id: String,
        reason: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Hook runners (pure functions, easy to unit-test without a browser).
// ---------------------------------------------------------------------------

/// Run all state plugins sequentially.
///
/// Each plugin receives the output of the previous plugin (or the original
/// input for the first plugin).  On plugin failure the **previous** output is
/// preserved and processing continues with the next plugin.
///
/// Returns `(final_state_json, audit_events)`.
pub fn run_state_hooks<'a>(
    initial_state_json: &str,
    plugins: impl Iterator<Item = &'a dyn StatePlugin>,
) -> (String, Vec<AuditEvent>) {
    let mut current = initial_state_json.to_string();
    let mut events = Vec::new();
    let timestamp = epoch_millis_u64();

    for plugin in plugins {
        match plugin.on_state(&current) {
            Ok(transformed) => {
                events.push(AuditEvent::PluginStateTransform {
                    plugin_id: plugin.plugin_id().to_string(),
                    success: true,
                    error_message: None,
                    timestamp,
                });
                current = transformed;
            }
            Err(err) => {
                events.push(AuditEvent::PluginStateTransform {
                    plugin_id: plugin.plugin_id().to_string(),
                    success: false,
                    error_message: Some(err),
                    timestamp,
                });
                // Non-fatal: keep `current` unchanged and continue.
            }
        }
    }

    (current, events)
}

/// Run all policy plugins sequentially.
///
/// Short-circuits on the first plugin that blocks or fails.  Plugin failures
/// are treated as blocks (fail-closed).
///
/// Returns `(outcome, audit_events)`.
pub fn run_policy_hooks<'a>(
    intent_json: &str,
    plugins: impl Iterator<Item = &'a dyn PolicyPlugin>,
) -> (PolicyHookOutcome, Vec<AuditEvent>) {
    let mut events = Vec::new();
    let timestamp = epoch_millis_u64();

    for plugin in plugins {
        let (allowed, reason) = match plugin.before_act(intent_json) {
            Ok(response_json) => parse_policy_response(&response_json),
            Err(err) => {
                // Treat execution failure as a block (fail-closed).
                (false, Some(format!("plugin execution failed: {err}")))
            }
        };

        events.push(AuditEvent::PluginPolicyDecision {
            plugin_id: plugin.plugin_id().to_string(),
            allowed,
            reason: reason.clone(),
            timestamp,
        });

        if !allowed {
            return (
                PolicyHookOutcome::Block {
                    plugin_id: plugin.plugin_id().to_string(),
                    reason,
                },
                events,
            );
        }
    }

    (PolicyHookOutcome::Allow, events)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse `{"allow": bool, "reason"?: string}` from a policy plugin response.
/// On parse failure, returns `(false, Some(error_msg))` (fail-closed).
fn parse_policy_response(json: &str) -> (bool, Option<String>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return (
            false,
            Some(format!("failed to parse policy plugin response: {json}")),
        );
    };

    let allowed = value
        .get("allow")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    let reason = value
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);

    (allowed, reason)
}

fn epoch_millis_u64() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    struct ConstStatePlugin {
        id: &'static str,
        output: &'static str,
    }

    impl StatePlugin for ConstStatePlugin {
        fn plugin_id(&self) -> &str {
            self.id
        }

        fn on_state(&self, _: &str) -> Result<String, String> {
            Ok(self.output.to_string())
        }
    }

    struct FailStatePlugin;

    impl StatePlugin for FailStatePlugin {
        fn plugin_id(&self) -> &str {
            "fail-state"
        }

        fn on_state(&self, _: &str) -> Result<String, String> {
            Err("plugin error".to_string())
        }
    }

    struct AllowPlugin;

    impl PolicyPlugin for AllowPlugin {
        fn plugin_id(&self) -> &str {
            "allow"
        }

        fn before_act(&self, _: &str) -> Result<String, String> {
            Ok(r#"{"allow":true}"#.to_string())
        }
    }

    struct BlockPlugin {
        id: &'static str,
    }

    impl PolicyPlugin for BlockPlugin {
        fn plugin_id(&self) -> &str {
            self.id
        }

        fn before_act(&self, _: &str) -> Result<String, String> {
            Ok(r#"{"allow":false,"reason":"test block"}"#.to_string())
        }
    }

    struct FailPolicyPlugin;

    impl PolicyPlugin for FailPolicyPlugin {
        fn plugin_id(&self) -> &str {
            "fail-policy"
        }

        fn before_act(&self, _: &str) -> Result<String, String> {
            Err("plugin execution failure".to_string())
        }
    }

    #[test]
    fn state_hooks_with_no_plugins_returns_original() {
        let plugins: Vec<Box<dyn StatePlugin>> = vec![];
        let (result, events) = run_state_hooks("{}", plugins.iter().map(|p| p.as_ref()));
        assert_eq!(result, "{}");
        assert!(events.is_empty());
    }

    #[test]
    fn state_hooks_transform_is_applied_in_order() {
        let plugins: Vec<Box<dyn StatePlugin>> = vec![
            Box::new(ConstStatePlugin {
                id: "a",
                output: "step-a",
            }),
            Box::new(ConstStatePlugin {
                id: "b",
                output: "step-b",
            }),
        ];
        let (result, events) = run_state_hooks("{}", plugins.iter().map(|p| p.as_ref()));
        assert_eq!(result, "step-b");
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn state_hooks_failure_preserves_previous_output() {
        let plugins: Vec<Box<dyn StatePlugin>> = vec![
            Box::new(ConstStatePlugin {
                id: "good",
                output: "good-output",
            }),
            Box::new(FailStatePlugin),
            Box::new(ConstStatePlugin {
                id: "after-fail",
                output: "after-fail-output",
            }),
        ];
        let (result, events) = run_state_hooks("{}", plugins.iter().map(|p| p.as_ref()));
        // FailStatePlugin keeps previous output "good-output"; after-fail transforms it.
        assert_eq!(result, "after-fail-output");
        assert_eq!(events.len(), 3);
        assert!(matches!(
            &events[1],
            AuditEvent::PluginStateTransform { success: false, .. }
        ));
    }

    #[test]
    fn policy_hooks_with_no_plugins_allows() {
        let plugins: Vec<Box<dyn PolicyPlugin>> = vec![];
        let (outcome, events) = run_policy_hooks("{}", plugins.iter().map(|p| p.as_ref()));
        assert_eq!(outcome, PolicyHookOutcome::Allow);
        assert!(events.is_empty());
    }

    #[test]
    fn policy_hooks_allow_passes() {
        let plugins: Vec<Box<dyn PolicyPlugin>> = vec![Box::new(AllowPlugin)];
        let (outcome, events) = run_policy_hooks("{}", plugins.iter().map(|p| p.as_ref()));
        assert_eq!(outcome, PolicyHookOutcome::Allow);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn policy_hooks_block_short_circuits() {
        let plugins: Vec<Box<dyn PolicyPlugin>> = vec![
            Box::new(AllowPlugin),
            Box::new(BlockPlugin { id: "b" }),
            Box::new(AllowPlugin), // never reached
        ];
        let (outcome, events) = run_policy_hooks("{}", plugins.iter().map(|p| p.as_ref()));
        assert!(matches!(outcome, PolicyHookOutcome::Block { .. }));
        assert_eq!(events.len(), 2); // allow + block
    }

    #[test]
    fn policy_hooks_failure_fails_closed() {
        let plugins: Vec<Box<dyn PolicyPlugin>> = vec![Box::new(FailPolicyPlugin)];
        let (outcome, events) = run_policy_hooks("{}", plugins.iter().map(|p| p.as_ref()));
        assert!(
            matches!(outcome, PolicyHookOutcome::Block { .. }),
            "failed policy plugin must fail closed"
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            AuditEvent::PluginPolicyDecision { allowed: false, .. }
        ));
    }

    #[test]
    fn parse_policy_response_handles_malformed_json() {
        let (allowed, reason) = parse_policy_response("not-json");
        assert!(!allowed);
        assert!(reason.is_some());
    }

    #[test]
    fn parse_policy_response_handles_missing_allow_field() {
        let (allowed, reason) = parse_policy_response(r#"{"foo":"bar"}"#);
        // Missing "allow" defaults to false (fail-closed)
        assert!(!allowed);
        assert!(reason.is_none());
    }
}
