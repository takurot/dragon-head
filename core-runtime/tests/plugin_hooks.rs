/// Tests for plugin hook integration in core-runtime (Issue #47).
///
/// These tests verify that:
/// 1. Core runtime with no plugins behaves identically to current behavior
/// 2. A state plugin can enrich SemanticState (via on_state transform)
/// 3. A policy plugin can block an action (before_act returns `{"allow": false}`)
/// 4. Plugin execution is recorded in the audit log
/// 5. Plugin failures in state hooks are non-fatal (observable but continue)
use core_runtime::{
    audit::AuditEvent,
    plugin_hooks::{PluginHookConfig, PolicyPlugin, StatePlugin},
};

// ---------------------------------------------------------------------------
// Mock plugin implementations (no Wasm needed for unit-level tests).
// ---------------------------------------------------------------------------

/// A state plugin that returns the input unchanged (identity transform).
struct IdentityStatePlugin;

impl StatePlugin for IdentityStatePlugin {
    fn plugin_id(&self) -> &str {
        "identity-state-plugin"
    }

    fn on_state(&self, state_json: &str) -> Result<String, String> {
        Ok(state_json.to_string())
    }
}

/// A state plugin that appends `"plugin_enriched": true` to the JSON object.
struct EnrichStatePlugin;

impl StatePlugin for EnrichStatePlugin {
    fn plugin_id(&self) -> &str {
        "enrich-state-plugin"
    }

    fn on_state(&self, state_json: &str) -> Result<String, String> {
        let mut obj: serde_json::Value =
            serde_json::from_str(state_json).map_err(|e| e.to_string())?;
        if let Some(map) = obj.as_object_mut() {
            map.insert("plugin_enriched".to_string(), serde_json::Value::Bool(true));
        }
        serde_json::to_string(&obj).map_err(|e| e.to_string())
    }
}

/// A state plugin that always fails (simulates a trapped Wasm plugin).
struct TrapStatePlugin {
    id: String,
}

impl StatePlugin for TrapStatePlugin {
    fn plugin_id(&self) -> &str {
        &self.id
    }

    fn on_state(&self, _state_json: &str) -> Result<String, String> {
        Err("plugin trapped: unreachable instruction executed".to_string())
    }
}

/// A policy plugin that always allows.
struct AllowPolicyPlugin;

impl PolicyPlugin for AllowPolicyPlugin {
    fn plugin_id(&self) -> &str {
        "allow-policy-plugin"
    }

    fn before_act(&self, _intent_json: &str) -> Result<String, String> {
        Ok(r#"{"allow":true}"#.to_string())
    }
}

/// A policy plugin that always blocks.
struct BlockPolicyPlugin {
    id: String,
}

impl PolicyPlugin for BlockPolicyPlugin {
    fn plugin_id(&self) -> &str {
        &self.id
    }

    fn before_act(&self, _intent_json: &str) -> Result<String, String> {
        Ok(r#"{"allow":false,"reason":"blocked by test plugin"}"#.to_string())
    }
}

/// A policy plugin that always fails (simulates trapped execution).
struct TrapPolicyPlugin;

impl PolicyPlugin for TrapPolicyPlugin {
    fn plugin_id(&self) -> &str {
        "trap-policy-plugin"
    }

    fn before_act(&self, _intent_json: &str) -> Result<String, String> {
        Err("plugin trapped".to_string())
    }
}

// ---------------------------------------------------------------------------
// Unit tests (no browser required).
// ---------------------------------------------------------------------------

#[test]
fn test_empty_plugin_config_is_backward_compat() {
    let config = PluginHookConfig::default();
    assert!(config.state_plugins.is_empty());
    assert!(config.policy_plugins.is_empty());
}

#[test]
fn test_state_plugin_config_holds_plugins() {
    let mut config = PluginHookConfig::default();
    config
        .state_plugins
        .push(Box::new(IdentityStatePlugin) as Box<dyn StatePlugin>);
    config
        .state_plugins
        .push(Box::new(EnrichStatePlugin) as Box<dyn StatePlugin>);
    assert_eq!(config.state_plugins.len(), 2);
}

#[test]
fn test_policy_plugin_config_holds_plugins() {
    let mut config = PluginHookConfig::default();
    config
        .policy_plugins
        .push(Box::new(AllowPolicyPlugin) as Box<dyn PolicyPlugin>);
    config.policy_plugins.push(Box::new(BlockPolicyPlugin {
        id: "block".to_string(),
    }) as Box<dyn PolicyPlugin>);
    assert_eq!(config.policy_plugins.len(), 2);
}

#[test]
fn test_identity_state_plugin_returns_unchanged_json() {
    let plugin = IdentityStatePlugin;
    let input = "{\"role\":\"document\",\"children\":[]}";
    assert_eq!(plugin.on_state(input).unwrap(), input);
}

#[test]
fn test_enrich_state_plugin_adds_field() {
    let plugin = EnrichStatePlugin;
    let input = "{\"role\":\"document\"}";
    let output = plugin.on_state(input).unwrap();
    let v: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(v["plugin_enriched"], serde_json::Value::Bool(true));
}

#[test]
fn test_trap_state_plugin_returns_error() {
    let plugin = TrapStatePlugin {
        id: "trap-state-plugin".to_string(),
    };
    assert!(plugin.on_state("{}").is_err());
}

#[test]
fn test_allow_policy_plugin_returns_allow() {
    let plugin = AllowPolicyPlugin;
    let output = plugin.before_act(r#"{"action":"click"}"#).unwrap();
    let v: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(v["allow"], serde_json::Value::Bool(true));
}

#[test]
fn test_block_policy_plugin_returns_deny() {
    let plugin = BlockPolicyPlugin {
        id: "block-p".to_string(),
    };
    let output = plugin.before_act(r#"{"action":"click"}"#).unwrap();
    let v: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(v["allow"], serde_json::Value::Bool(false));
}

// ---------------------------------------------------------------------------
// run_state_hooks / run_policy_hooks unit tests
// ---------------------------------------------------------------------------

use core_runtime::plugin_hooks::{run_policy_hooks, run_state_hooks, PolicyHookOutcome};

#[test]
fn test_run_state_hooks_no_plugins_returns_input_unchanged() {
    let plugins: Vec<Box<dyn StatePlugin>> = vec![];
    let input = "{\"role\":\"document\"}";
    let (result, events) = run_state_hooks(input, plugins.iter().map(|p| p.as_ref()));
    assert_eq!(result, input);
    assert!(events.is_empty());
}

#[test]
fn test_run_state_hooks_identity_plugin_returns_same() {
    let plugins: Vec<Box<dyn StatePlugin>> = vec![Box::new(IdentityStatePlugin)];
    let input = "{\"role\":\"document\"}";
    let (result, events) = run_state_hooks(input, plugins.iter().map(|p| p.as_ref()));
    assert_eq!(result, input);
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        AuditEvent::PluginStateTransform { success: true, .. }
    ));
}

#[test]
fn test_run_state_hooks_failing_plugin_is_nonfatal() {
    let plugins: Vec<Box<dyn StatePlugin>> = vec![
        Box::new(TrapStatePlugin {
            id: "trap".to_string(),
        }),
        Box::new(IdentityStatePlugin),
    ];
    let input = "{\"role\":\"document\"}";
    let (result, events) = run_state_hooks(input, plugins.iter().map(|p| p.as_ref()));
    // Input unchanged because trap plugin failed, identity continues with original
    assert_eq!(result, input);
    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[0],
        AuditEvent::PluginStateTransform { success: false, plugin_id, .. } if plugin_id == "trap"
    ));
    assert!(matches!(
        &events[1],
        AuditEvent::PluginStateTransform { success: true, .. }
    ));
}

#[test]
fn test_run_policy_hooks_no_plugins_allows() {
    let plugins: Vec<Box<dyn PolicyPlugin>> = vec![];
    let intent = r#"{"action":"click","target":"btn","url":"http://example.com"}"#;
    let (outcome, events) = run_policy_hooks(intent, plugins.iter().map(|p| p.as_ref()));
    assert!(matches!(outcome, PolicyHookOutcome::Allow));
    assert!(events.is_empty());
}

#[test]
fn test_run_policy_hooks_allow_plugin_passes() {
    let plugins: Vec<Box<dyn PolicyPlugin>> = vec![Box::new(AllowPolicyPlugin)];
    let intent = r#"{"action":"click","target":"btn","url":"http://example.com"}"#;
    let (outcome, events) = run_policy_hooks(intent, plugins.iter().map(|p| p.as_ref()));
    assert!(matches!(outcome, PolicyHookOutcome::Allow));
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        AuditEvent::PluginPolicyDecision { allowed: true, .. }
    ));
}

#[test]
fn test_run_policy_hooks_block_plugin_blocks() {
    let plugins: Vec<Box<dyn PolicyPlugin>> = vec![Box::new(BlockPolicyPlugin {
        id: "block-test".to_string(),
    })];
    let intent = r#"{"action":"click","target":"btn","url":"http://example.com"}"#;
    let (outcome, events) = run_policy_hooks(intent, plugins.iter().map(|p| p.as_ref()));
    assert!(matches!(outcome, PolicyHookOutcome::Block { .. }));
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        AuditEvent::PluginPolicyDecision { allowed: false, .. }
    ));
}

#[test]
fn test_run_policy_hooks_short_circuits_on_block() {
    let plugins: Vec<Box<dyn PolicyPlugin>> = vec![
        Box::new(BlockPolicyPlugin {
            id: "block-first".to_string(),
        }),
        Box::new(AllowPolicyPlugin), // should not be reached
    ];
    let intent = r#"{"action":"click","target":"btn","url":"http://example.com"}"#;
    let (outcome, events) = run_policy_hooks(intent, plugins.iter().map(|p| p.as_ref()));
    assert!(matches!(outcome, PolicyHookOutcome::Block { .. }));
    // Only the first plugin should be evaluated
    assert_eq!(events.len(), 1);
}

#[test]
fn test_run_policy_hooks_trap_fails_closed() {
    let plugins: Vec<Box<dyn PolicyPlugin>> = vec![Box::new(TrapPolicyPlugin)];
    let intent = r#"{"action":"click","target":"btn","url":"http://example.com"}"#;
    let (outcome, events) = run_policy_hooks(intent, plugins.iter().map(|p| p.as_ref()));
    // Policy hook failure must fail closed (block)
    assert!(
        matches!(outcome, PolicyHookOutcome::Block { .. }),
        "trap in policy plugin must fail closed"
    );
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        AuditEvent::PluginPolicyDecision { allowed: false, .. }
    ));
}

// ---------------------------------------------------------------------------
// Browser-based integration tests (require chrome).
// ---------------------------------------------------------------------------

#[test]
fn test_no_plugins_unchanged_browser_behavior() {
    if test_bench_support::should_skip_browser_tests() {
        return;
    }

    let client = core_runtime::BrowserClient::new().expect("browser client");
    let page = client.new_page().expect("page");

    page.navigate("data:text/html,<html><body>hello</body></html>")
        .expect("navigate");

    let state = page
        .capture_semantic_state(core_runtime::sre::LoadProfile::Minimal)
        .expect("capture state");

    assert!(!state.root().role.is_empty());
}

#[test]
fn test_block_policy_plugin_prevents_action_in_browser() {
    if test_bench_support::should_skip_browser_tests() {
        return;
    }

    let mut config = PluginHookConfig::default();
    config.policy_plugins.push(Box::new(BlockPolicyPlugin {
        id: "block-browser-test".to_string(),
    }));

    let client = core_runtime::BrowserClient::new_with_plugin_hooks(config).expect("browser");
    let page = client.new_page().expect("page");

    let html = r#"<html><body><button id="btn">Click</button></body></html>"#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url).expect("navigate");

    let state = page
        .capture_semantic_state(core_runtime::sre::LoadProfile::Interactive)
        .expect("capture");
    let button = find_button(state.root()).expect("button in state");

    let result = page.act(Some(button.backend_node_id), None, "click", None);
    assert!(result.is_err(), "block plugin must reject the action");
}

#[test]
fn test_policy_plugin_decision_recorded_in_audit_browser() {
    if test_bench_support::should_skip_browser_tests() {
        return;
    }

    let mut config = PluginHookConfig::default();
    config.policy_plugins.push(Box::new(BlockPolicyPlugin {
        id: "audit-block-plugin".to_string(),
    }));

    let client = core_runtime::BrowserClient::new_with_plugin_hooks(config).expect("browser");
    let page = client.new_page().expect("page");

    let html = r#"<html><body><button>Go</button></body></html>"#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url).expect("navigate");

    let state = page
        .capture_semantic_state(core_runtime::sre::LoadProfile::Interactive)
        .expect("capture");
    let button = find_button(state.root()).expect("button in state");

    page.clear_audit_events();
    let _ = page.act(Some(button.backend_node_id), None, "click", None);

    let events = page.audit_events();
    let has_plugin_decision = events.iter().any(|e| {
        matches!(
            e,
            AuditEvent::PluginPolicyDecision { plugin_id, .. }
            if plugin_id == "audit-block-plugin"
        )
    });
    assert!(
        has_plugin_decision,
        "audit log must contain PluginPolicyDecision for the block plugin; got: {events:?}"
    );
}

#[test]
fn test_state_plugin_failure_is_nonfatal_in_browser() {
    if test_bench_support::should_skip_browser_tests() {
        return;
    }

    let mut config = PluginHookConfig::default();
    config.state_plugins.push(Box::new(TrapStatePlugin {
        id: "trap-state-browser".to_string(),
    }));

    let client = core_runtime::BrowserClient::new_with_plugin_hooks(config).expect("browser");
    let page = client.new_page().expect("page");

    page.navigate("data:text/html,<html><body>hello</body></html>")
        .expect("navigate");

    // capture_semantic_state must NOT fail even though the state plugin traps.
    let result = page.capture_semantic_state(core_runtime::sre::LoadProfile::Minimal);
    assert!(
        result.is_ok(),
        "state plugin failure must be non-fatal: {:?}",
        result
    );
}

#[test]
fn test_state_plugin_failure_recorded_in_audit_browser() {
    if test_bench_support::should_skip_browser_tests() {
        return;
    }

    let mut config = PluginHookConfig::default();
    config.state_plugins.push(Box::new(TrapStatePlugin {
        id: "trap-state-audit-browser".to_string(),
    }));

    let client = core_runtime::BrowserClient::new_with_plugin_hooks(config).expect("browser");
    let page = client.new_page().expect("page");

    page.navigate("data:text/html,<html><body>test</body></html>")
        .expect("navigate");
    page.clear_audit_events();

    let _ = page.capture_semantic_state(core_runtime::sre::LoadProfile::Minimal);

    let events = page.audit_events();
    let has_failure = events.iter().any(|e| {
        matches!(
            e,
            AuditEvent::PluginStateTransform {
                plugin_id,
                success: false,
                ..
            } if plugin_id == "trap-state-audit-browser"
        )
    });
    assert!(
        has_failure,
        "audit log must record state plugin failure: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn find_button(node: &core_runtime::sre::SemanticNode) -> Option<&core_runtime::sre::SemanticNode> {
    if node.role == "button" {
        return Some(node);
    }
    for child in &node.children {
        if let Some(found) = find_button(child) {
            return Some(found);
        }
    }
    None
}
