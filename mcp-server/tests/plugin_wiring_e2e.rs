//! Production-composition-root E2E tests for ISSUE-303: proves that a plugin configured via
//! `config.toml` and wired through `mcp_server::config::load_configured_plugins` +
//! `mcp_server::plugins::build_plugin_hook_config` actually runs inside a real
//! `core_runtime::BrowserClient` built with `new_with_chrome_path_and_plugin_hooks` — the exact
//! composition `main.rs` performs at startup — rather than a `PluginHookConfig` built by hand
//! inside the test (that pipeline is already covered by `core-runtime/tests/plugin_hooks.rs` and
//! `core-runtime/tests/repro_plugin_pipeline.rs`; what was previously untested is the
//! `config.toml` -> `PluginHost::load_plugin` -> `BrowserClient` composition step itself).

use core_runtime::audit::AuditEvent;
use core_runtime::sre::LoadProfile;
use core_runtime::BrowserClient;
use ed25519_dalek::{Signer, SigningKey};
use mcp_server::config;
use plugin_host::{
    Capability, ExtensionPoint, PluginManifest, SbomComponent, SbomDocument, SignatureBlock,
};

fn sample_sbom() -> SbomDocument {
    SbomDocument {
        format: "cyclonedx-1.5".to_string(),
        components: vec![SbomComponent {
            name: "plugin-wiring-e2e-fixture".to_string(),
            version: "0.1.0".to_string(),
            license: Some("MIT".to_string()),
        }],
    }
}

fn sign_manifest(
    mut manifest: PluginManifest,
    wasm: &[u8],
    signing_key: &SigningKey,
    key_id: &str,
) -> PluginManifest {
    let payload = plugin_host::signature_payload(&manifest, wasm).unwrap();
    let signature = signing_key.sign(&payload);
    manifest.signature = Some(SignatureBlock {
        key_id: key_id.to_string(),
        signature_hex: hex::encode(signature.to_bytes()),
    });
    manifest
}

/// `on_state`: echoes input unchanged. `before_act`: always returns `{"allow":false}`.
fn echo_state_block_act_wasm() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
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
            (func (export "before_act")
                  (param $in_ptr i32) (param $in_len i32)
                  (param $out_ptr i32) (param $out_len_ptr i32)
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 0)) (i32.const 123))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 1)) (i32.const 34))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 2)) (i32.const 97))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 3)) (i32.const 108))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 4)) (i32.const 108))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 5)) (i32.const 111))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 6)) (i32.const 119))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 7)) (i32.const 34))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 8)) (i32.const 58))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 9)) (i32.const 102))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 10)) (i32.const 97))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 11)) (i32.const 108))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 12)) (i32.const 115))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 13)) (i32.const 101))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 14)) (i32.const 125))
                (i32.store (local.get $out_len_ptr) (i32.const 15))
            )
        )"#,
    )
    .unwrap()
}

/// Writes a signed `[[plugin_trust_keys]]`/`[[plugins]]` `config.toml` (plus the manifest/wasm
/// files it references) declaring both `OnState` and `BeforeAct`, then runs the exact
/// `config::load_configured_plugins` -> `plugins::build_plugin_hook_config` composition `main.rs`
/// performs at startup.
fn build_hooks_from_config_toml(
    dir: &std::path::Path,
) -> (core_runtime::PluginHookConfig, plugin_host::PluginHost) {
    let signing_key = SigningKey::from_bytes(&[77u8; 32]);
    let wasm = echo_state_block_act_wasm();
    let manifest = PluginManifest {
        plugin_id: "e2e-composition.plugin".to_string(),
        version: "0.1.0".to_string(),
        entry_points: vec![ExtensionPoint::OnState, ExtensionPoint::BeforeAct],
        capabilities: vec![Capability::ReadState],
        signature: None,
        sbom: sample_sbom(),
    };
    let manifest = sign_manifest(manifest, &wasm, &signing_key, "e2e-key");

    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join("plugin.wasm"), &wasm).unwrap();
    let config_path = dir.join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[[plugin_trust_keys]]\nid = \"e2e-key\"\npublic_key_hex = \"{}\"\n\n\
             [[plugins]]\nmanifest = \"manifest.json\"\nwasm = \"plugin.wasm\"\n",
            hex::encode(signing_key.verifying_key().to_bytes())
        ),
    )
    .unwrap();

    let file_config = config::load_config_file(&config_path).unwrap().unwrap();
    let (key_registry, configured) =
        config::load_configured_plugins(Some(&config_path), Some(&file_config)).unwrap();
    mcp_server::plugins::build_plugin_hook_config(key_registry, configured).unwrap()
}

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

#[test]
fn config_driven_plugin_transforms_state_and_is_audited() {
    if test_bench_support::should_skip_browser_tests() {
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let (plugin_hooks, _plugin_host) = build_hooks_from_config_toml(dir.path());
    assert_eq!(plugin_hooks.state_plugins.len(), 1);
    assert_eq!(plugin_hooks.policy_plugins.len(), 1);

    let client =
        BrowserClient::new_with_chrome_path_and_plugin_hooks(None, plugin_hooks).expect("browser");
    let page = client.new_page().expect("page");

    // Small fixture so the serialized SemanticState stays under the Wasm ABI's 16 KiB input cap
    // (see docs/plugins.md) — otherwise `on_state` would fail non-fatally and this test would
    // pass for the wrong reason (no transform attempted, rather than transform succeeding).
    page.navigate("data:text/html,<html><body>hello</body></html>")
        .expect("navigate");

    page.clear_audit_events();
    let state = page
        .capture_semantic_state(LoadProfile::Minimal)
        .expect("capture state");
    assert!(!state.root().role.is_empty(), "state must still be usable");

    let events = page.audit_events();
    let transformed = events.iter().any(|event| {
        matches!(
            event,
            AuditEvent::PluginStateTransform {
                plugin_id,
                success: true,
                ..
            } if plugin_id == "e2e-composition.plugin"
        )
    });
    assert!(
        transformed,
        "config-loaded plugin must run on_state and be audited as a success; got: {events:?}"
    );
}

#[test]
fn config_driven_plugin_vetoes_action_and_is_audited() {
    if test_bench_support::should_skip_browser_tests() {
        return;
    }

    let dir = tempfile::tempdir().unwrap();
    let (plugin_hooks, _plugin_host) = build_hooks_from_config_toml(dir.path());

    let client =
        BrowserClient::new_with_chrome_path_and_plugin_hooks(None, plugin_hooks).expect("browser");
    let page = client.new_page().expect("page");

    let html = r#"<html><body><button id="btn">Click</button></body></html>"#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url).expect("navigate");

    let state = page
        .capture_semantic_state(LoadProfile::Interactive)
        .expect("capture");
    let button = find_button(state.root()).expect("button in state");

    page.clear_audit_events();
    let result = page.act(Some(button.backend_node_id), None, "click", None);
    assert!(
        result.is_err(),
        "config-loaded before_act plugin returning {{\"allow\":false}} must block the action"
    );

    let events = page.audit_events();
    let vetoed = events.iter().any(|event| {
        matches!(
            event,
            AuditEvent::PluginPolicyDecision {
                plugin_id,
                allowed: false,
                ..
            } if plugin_id == "e2e-composition.plugin"
        )
    });
    assert!(
        vetoed,
        "config-loaded plugin's veto must be audited; got: {events:?}"
    );
}
