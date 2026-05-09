/// Reproduction / integration tests for ISSUE-05: plugin-host execution entry
/// point wired into the core-runtime pipeline.
///
/// Before the fix, `core-runtime::plugin_hooks` only had abstract trait objects
/// (`StatePlugin`, `PolicyPlugin`).  There was no concrete adapter that wired a
/// real `LoadedPlugin` / `PluginRuntime` from `plugin-host` into those traits,
/// so callers had no supported path to inject Wasm-backed plugins.
///
/// The fix introduced:
/// 1. `WasmStatePlugin`  — wraps a `LoadedPlugin`, implements `StatePlugin`.
/// 2. `WasmPolicyPlugin` — wraps a `LoadedPlugin`, implements `PolicyPlugin`.
///
/// Both adapters are gated on the `plugin-host` feature of `core-runtime`.
/// They enforce capability checks and fail-closed semantics already required by
/// the hook runners, but now backed by a live Wasm instance.
use core_runtime::plugin_hooks::{
    run_policy_hooks, run_state_hooks, PolicyHookOutcome, WasmPolicyPlugin, WasmStatePlugin,
};
use ed25519_dalek::{Signer, SigningKey};
use plugin_host::{
    signature_payload, Capability, ExtensionPoint, KeyRegistry, PluginHost, PluginManifest,
    PluginPackage, SbomComponent, SbomDocument, SignatureBlock,
};

// ---------------------------------------------------------------------------
// Shared WAT fixtures
// ---------------------------------------------------------------------------

/// Wasm that echoes input as output (for on_state) and always returns
/// `{"allow":true}` for before_act.
fn echo_wasm() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (memory (export "memory") 1)

            ;; on_state: echo-copy input to output buffer
            (func (export "on_state")
                  (param $in_ptr i32) (param $in_len i32)
                  (param $out_ptr i32) (param $out_len_ptr i32)
                (local $i i32)
                (local.set $i (i32.const 0))
                (block $break
                    (loop $loop
                        (br_if $break (i32.ge_u (local.get $i) (local.get $in_len)))
                        (i32.store8
                            (i32.add (local.get $out_ptr) (local.get $i))
                            (i32.load8_u (i32.add (local.get $in_ptr) (local.get $i))))
                        (local.set $i (i32.add (local.get $i) (i32.const 1)))
                        (br $loop)))
                (i32.store (local.get $out_len_ptr) (local.get $in_len))
            )

            ;; before_act: always allow
            (func (export "before_act")
                  (param $in_ptr i32) (param $in_len i32)
                  (param $out_ptr i32) (param $out_len_ptr i32)
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 0))  (i32.const 123))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 1))  (i32.const 34))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 2))  (i32.const 97))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 3))  (i32.const 108))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 4))  (i32.const 108))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 5))  (i32.const 111))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 6))  (i32.const 119))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 7))  (i32.const 34))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 8))  (i32.const 58))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 9))  (i32.const 116))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 10)) (i32.const 114))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 11)) (i32.const 117))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 12)) (i32.const 101))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 13)) (i32.const 125))
                (i32.store (local.get $out_len_ptr) (i32.const 14))
            )
        )"#,
    )
    .expect("failed to build echo wasm fixture")
}

/// Wasm whose `before_act` always returns `{"allow":false}`.
fn block_wasm() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (func (export "before_act")
                  (param $in_ptr i32) (param $in_len i32)
                  (param $out_ptr i32) (param $out_len_ptr i32)
                ;; {"allow":false} = 14 bytes
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 0))  (i32.const 123))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 1))  (i32.const 34))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 2))  (i32.const 97))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 3))  (i32.const 108))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 4))  (i32.const 108))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 5))  (i32.const 111))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 6))  (i32.const 119))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 7))  (i32.const 34))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 8))  (i32.const 58))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 9))  (i32.const 102))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 10)) (i32.const 97))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 11)) (i32.const 108))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 12)) (i32.const 115))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 13)) (i32.const 101))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 14)) (i32.const 125))
                (i32.store (local.get $out_len_ptr) (i32.const 15))
            )
        )"#,
    )
    .expect("failed to build block wasm fixture")
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn make_registry_and_key() -> (KeyRegistry, SigningKey, String) {
    let signing_key = SigningKey::from_bytes(&[99u8; 32]);
    let key_id = "pipeline-test-key".to_string();

    let mut registry = KeyRegistry::default();
    registry
        .register_hex_ed25519(
            &key_id,
            &hex::encode(signing_key.verifying_key().to_bytes()),
        )
        .expect("must register key");

    (registry, signing_key, key_id)
}

fn sample_sbom() -> SbomDocument {
    SbomDocument {
        format: "cyclonedx-1.5".to_string(),
        components: vec![SbomComponent {
            name: "pipeline-fixture".to_string(),
            version: "0.1.0".to_string(),
            license: Some("MIT".to_string()),
        }],
    }
}

fn build_and_sign_package(
    entry_points: Vec<ExtensionPoint>,
    capabilities: Vec<Capability>,
    wasm: Vec<u8>,
    signing_key: &SigningKey,
    key_id: &str,
) -> PluginPackage {
    let mut manifest = PluginManifest {
        plugin_id: "pipeline.test.plugin".to_string(),
        version: "0.1.0".to_string(),
        entry_points,
        capabilities,
        signature: None,
        sbom: sample_sbom(),
    };

    let payload = signature_payload(&manifest, &wasm).expect("payload must build");
    let sig = signing_key.sign(&payload);
    manifest.signature = Some(SignatureBlock {
        key_id: key_id.to_string(),
        signature_hex: hex::encode(sig.to_bytes()),
    });

    PluginPackage {
        manifest,
        wasm_module: wasm,
    }
}

// ---------------------------------------------------------------------------
// ISSUE-05 regression: WasmStatePlugin wires into run_state_hooks
// ---------------------------------------------------------------------------

/// BUG REGRESSION (ISSUE-05): A Wasm-backed state plugin must be loadable and
/// injectable into `run_state_hooks` via the `WasmStatePlugin` adapter.
///
/// Before the fix there was no `WasmStatePlugin` type — callers had no supported
/// path to use a real Wasm plugin in the state hook pipeline.
#[test]
fn wasm_state_plugin_echoes_state_json_through_pipeline() {
    let (registry, signing_key, key_id) = make_registry_and_key();
    let wasm = echo_wasm();
    let package = build_and_sign_package(
        vec![ExtensionPoint::OnState],
        vec![Capability::ReadState],
        wasm,
        &signing_key,
        &key_id,
    );

    let host = PluginHost::new(registry);
    let loaded = host.load_plugin(&package).expect("plugin must load");

    let plugin = WasmStatePlugin::new(loaded).expect("adapter must be created");

    let initial_state = r#"{"url":"https://example.com","elements":[]}"#;
    let plugins: Vec<Box<dyn core_runtime::plugin_hooks::StatePlugin>> = vec![Box::new(plugin)];
    let (result, events) = run_state_hooks(initial_state, plugins.iter().map(|p| p.as_ref()));

    let parsed: serde_json::Value = serde_json::from_str(&result).expect("must be valid JSON");
    assert_eq!(parsed["url"], "https://example.com");
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        core_runtime::audit::AuditEvent::PluginStateTransform { success: true, .. }
    ));
}

/// BUG REGRESSION (ISSUE-05): A Wasm-backed policy plugin with `{"allow":true}`
/// must pass through `run_policy_hooks` as `PolicyHookOutcome::Allow`.
#[test]
fn wasm_policy_plugin_allow_passes_pipeline() {
    let (registry, signing_key, key_id) = make_registry_and_key();
    let wasm = echo_wasm();
    let package = build_and_sign_package(
        vec![ExtensionPoint::BeforeAct],
        vec![],
        wasm,
        &signing_key,
        &key_id,
    );

    let host = PluginHost::new(registry);
    let loaded = host.load_plugin(&package).expect("plugin must load");

    let plugin = WasmPolicyPlugin::new(loaded).expect("adapter must be created");

    let intent = r#"{"action":"click","target":"submit-btn"}"#;
    let plugins: Vec<Box<dyn core_runtime::plugin_hooks::PolicyPlugin>> = vec![Box::new(plugin)];
    let (outcome, events) = run_policy_hooks(intent, plugins.iter().map(|p| p.as_ref()));

    assert_eq!(
        outcome,
        PolicyHookOutcome::Allow,
        "allow-returning Wasm plugin must produce Allow outcome"
    );
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        core_runtime::audit::AuditEvent::PluginPolicyDecision { allowed: true, .. }
    ));
}

/// BUG REGRESSION (ISSUE-05): A Wasm-backed policy plugin that returns
/// `{"allow":false}` must block the action (fail-closed).
#[test]
fn wasm_policy_plugin_block_fails_closed() {
    let (registry, signing_key, key_id) = make_registry_and_key();
    let wasm = block_wasm();
    let package = build_and_sign_package(
        vec![ExtensionPoint::BeforeAct],
        vec![],
        wasm,
        &signing_key,
        &key_id,
    );

    let host = PluginHost::new(registry);
    let loaded = host.load_plugin(&package).expect("plugin must load");

    let plugin = WasmPolicyPlugin::new(loaded).expect("adapter must be created");

    let intent = r#"{"action":"transfer","amount":10000}"#;
    let plugins: Vec<Box<dyn core_runtime::plugin_hooks::PolicyPlugin>> = vec![Box::new(plugin)];
    let (outcome, _events) = run_policy_hooks(intent, plugins.iter().map(|p| p.as_ref()));

    assert!(
        matches!(outcome, PolicyHookOutcome::Block { .. }),
        "block-returning Wasm plugin must produce Block outcome, got {outcome:?}"
    );
}

/// Calling `WasmStatePlugin::new` on a plugin without `ReadState` capability
/// must fail (capability check at adapter construction).
#[test]
fn wasm_state_plugin_rejects_missing_read_state_capability() {
    let (registry, signing_key, key_id) = make_registry_and_key();
    let wasm = echo_wasm();
    let package = build_and_sign_package(
        vec![ExtensionPoint::OnState],
        vec![], // ReadState intentionally omitted
        wasm,
        &signing_key,
        &key_id,
    );

    let host = PluginHost::new(registry);
    let loaded = host.load_plugin(&package).expect("plugin must load");

    let result = WasmStatePlugin::new(loaded);
    assert!(
        result.is_err(),
        "WasmStatePlugin must reject a plugin without ReadState capability"
    );
}

/// Calling `WasmPolicyPlugin::new` on a plugin without the `BeforeAct` entry
/// point must fail (extension point check at adapter construction).
#[test]
fn wasm_policy_plugin_rejects_missing_before_act_entry_point() {
    let (registry, signing_key, key_id) = make_registry_and_key();
    let wasm = echo_wasm();
    // Declare only OnState — BeforeAct is missing
    let package = build_and_sign_package(
        vec![ExtensionPoint::OnState],
        vec![Capability::ReadState],
        wasm,
        &signing_key,
        &key_id,
    );

    let host = PluginHost::new(registry);
    let loaded = host.load_plugin(&package).expect("plugin must load");

    let result = WasmPolicyPlugin::new(loaded);
    assert!(
        result.is_err(),
        "WasmPolicyPlugin must reject a plugin without BeforeAct entry point"
    );
}
