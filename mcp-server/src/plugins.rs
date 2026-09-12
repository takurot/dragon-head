//! Wires locally configured, signed Wasm plugins into a `core_runtime::PluginHookConfig`
//! (ISSUE-303). See `docs/plugins.md`.
//!
//! Composition-root only: file I/O and manifest/wasm-bytes resolution live in
//! `crate::config::load_configured_plugins`; this module owns `plugin_host::PluginHost`
//! verification (manifest schema, SBOM, ed25519 signature, Wasm module validation,
//! declared-export existence) and the `WasmStatePlugin`/`WasmPolicyPlugin` adapter construction
//! that turns a verified `LoadedPlugin` into the trait objects `core_runtime::BrowserClient`
//! actually calls (`core_runtime::browser` already invokes `run_state_hooks`/`run_policy_hooks`
//! against whatever `PluginHookConfig` it was constructed with — the missing piece was always
//! this composition step, not the runtime pipeline).
//!
//! Every failure here is explicit and fatal: an enabled-but-misconfigured plugin fails
//! `dragon-head-mcp` startup (and `--doctor`) rather than being silently dropped, per ISSUE-303's
//! explicit requirement. Use `enabled = false` in `config.toml` to take a broken plugin out of
//! the startup path entirely — `config::load_configured_plugins` never even reads its files.

use std::collections::HashMap;
use std::path::PathBuf;

use core_runtime::plugin_hooks::{
    PluginAdapterError, PluginHookConfig, WasmPolicyPlugin, WasmStatePlugin,
};
use plugin_host::{ExtensionPoint, KeyRegistry, PluginError, PluginHost};

use crate::config::ConfiguredPlugin;

#[derive(Debug, thiserror::Error)]
pub enum PluginWiringError {
    #[error("plugin manifest {manifest_path} failed to load/verify: {source}")]
    Load {
        manifest_path: PathBuf,
        #[source]
        source: PluginError,
    },
    #[error("plugin '{plugin_id}' ({manifest_path}) failed adapter construction: {source}")]
    Adapter {
        plugin_id: String,
        manifest_path: PathBuf,
        #[source]
        source: PluginAdapterError,
    },
    #[error(
        "plugin '{plugin_id}' ({manifest_path}) declares no extension point dragon-head-mcp \
         wires (OnState/BeforeAct) — an enabled plugin that declares only Connector (unsupported; \
         see ISSUE-155) or no entry points at all would silently do nothing, which ISSUE-303 \
         requires this to reject explicitly; disable it or add OnState/BeforeAct"
    )]
    UnsupportedExtensionPoint {
        plugin_id: String,
        manifest_path: PathBuf,
    },
    #[error(
        "duplicate plugin_id '{plugin_id}' (first declared in {first_manifest_path}, again in \
         {manifest_path})"
    )]
    DuplicatePluginId {
        plugin_id: String,
        manifest_path: PathBuf,
        first_manifest_path: PathBuf,
    },
}

/// Verifies and wires every configured plugin into a `PluginHookConfig`.
///
/// For each entry: `PluginHost::load_plugin` performs full verification (manifest schema, SBOM,
/// ed25519 signature, Wasm module validation, declared-export existence). A successfully loaded
/// plugin's declared `entry_points` are wired: `OnState` -> `WasmStatePlugin` (`state_plugins`),
/// `BeforeAct` -> `WasmPolicyPlugin` (`policy_plugins`) — each gets its own Wasm instance/store,
/// so a plugin declaring both does not share state between the two hooks. `Connector` is not yet
/// wired (ISSUE-155 tracks marketplace/Connector distribution): a plugin that declares it
/// alongside a supported extension point gets a `[PLUGIN][WARN]` stderr line (its other hooks
/// stay active); a plugin that declares *only* `Connector` (or no entry points at all) is a hard
/// error, because an enabled plugin wiring zero hooks is exactly the silent no-op ISSUE-303
/// forbids.
///
/// Any verification/adapter-construction failure returns `Err` immediately (fail-fast): a
/// misconfigured *enabled* plugin must block startup, not be silently skipped. Disabling it in
/// `config.toml` (`enabled = false`) is the supported way to take it out of the startup path —
/// see `config::load_configured_plugins`, which never reads a disabled entry's files at all.
///
/// **Side effect**: this instantiates a live Wasm store for every wired plugin (via
/// `WasmStatePlugin::new`/`WasmPolicyPlugin::new`), which runs the module's `start` section if it
/// declares one. Callers that only want to validate configuration without running plugin code
/// (there is currently no such path — `--doctor` intentionally calls this same function so that
/// "adapter construction succeeds" is actually exercised before normal startup, per ISSUE-303's
/// acceptance criteria) should be aware of this.
pub fn build_plugin_hook_config(
    key_registry: KeyRegistry,
    configured: Vec<ConfiguredPlugin>,
) -> Result<PluginHookConfig, PluginWiringError> {
    let host = PluginHost::new(key_registry);
    let mut config = PluginHookConfig::default();
    let mut first_manifest_paths: HashMap<String, PathBuf> = HashMap::new();

    for entry in configured {
        let loaded =
            host.load_plugin(&entry.package)
                .map_err(|source| PluginWiringError::Load {
                    manifest_path: entry.manifest_path.clone(),
                    source,
                })?;

        let plugin_id = loaded.manifest().plugin_id.clone();
        if let Some(first_manifest_path) = first_manifest_paths.get(&plugin_id) {
            return Err(PluginWiringError::DuplicatePluginId {
                plugin_id,
                manifest_path: entry.manifest_path,
                first_manifest_path: first_manifest_path.clone(),
            });
        }
        first_manifest_paths.insert(plugin_id.clone(), entry.manifest_path.clone());

        let entry_points = loaded.manifest().entry_points.clone();
        let mut wired = 0usize;

        if entry_points.contains(&ExtensionPoint::OnState) {
            let plugin = WasmStatePlugin::new(loaded.clone()).map_err(|source| {
                PluginWiringError::Adapter {
                    plugin_id: plugin_id.clone(),
                    manifest_path: entry.manifest_path.clone(),
                    source,
                }
            })?;
            config.state_plugins.push(Box::new(plugin));
            wired += 1;
        }

        if entry_points.contains(&ExtensionPoint::BeforeAct) {
            let plugin = WasmPolicyPlugin::new(loaded.clone()).map_err(|source| {
                PluginWiringError::Adapter {
                    plugin_id: plugin_id.clone(),
                    manifest_path: entry.manifest_path.clone(),
                    source,
                }
            })?;
            config.policy_plugins.push(Box::new(plugin));
            wired += 1;
        }

        if entry_points.contains(&ExtensionPoint::Connector) && wired > 0 {
            eprintln!(
                "[PLUGIN][WARN] plugin '{plugin_id}' declares Connector, which \
                 dragon-head-mcp does not yet wire (see ISSUE-155); its other declared \
                 extension points remain active."
            );
        }

        if wired == 0 {
            return Err(PluginWiringError::UnsupportedExtensionPoint {
                plugin_id,
                manifest_path: entry.manifest_path,
            });
        }
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_runtime::plugin_hooks::{run_policy_hooks, run_state_hooks, PolicyHookOutcome};
    use ed25519_dalek::{Signer, SigningKey};
    use plugin_host::{PluginManifest, PluginPackage, SbomComponent, SbomDocument, SignatureBlock};
    use std::path::PathBuf;

    fn key_registry_with(signing_key: &SigningKey, key_id: &str) -> KeyRegistry {
        let mut registry = KeyRegistry::default();
        registry
            .register_hex_ed25519(key_id, &hex::encode(signing_key.verifying_key().to_bytes()))
            .unwrap();
        registry
    }

    fn sample_sbom() -> SbomDocument {
        SbomDocument {
            format: "cyclonedx-1.5".to_string(),
            components: vec![SbomComponent {
                name: "plugins-test-fixture".to_string(),
                version: "0.1.0".to_string(),
                license: Some("MIT".to_string()),
            }],
        }
    }

    fn signed_package(
        plugin_id: &str,
        entry_points: Vec<ExtensionPoint>,
        capabilities: Vec<plugin_host::Capability>,
        wasm: Vec<u8>,
        signing_key: &SigningKey,
        key_id: &str,
    ) -> PluginPackage {
        let mut manifest = PluginManifest {
            plugin_id: plugin_id.to_string(),
            version: "0.1.0".to_string(),
            entry_points,
            capabilities,
            signature: None,
            sbom: sample_sbom(),
        };
        let payload = plugin_host::signature_payload(&manifest, &wasm).unwrap();
        let signature = signing_key.sign(&payload);
        manifest.signature = Some(SignatureBlock {
            key_id: key_id.to_string(),
            signature_hex: hex::encode(signature.to_bytes()),
        });
        PluginPackage {
            manifest,
            wasm_module: wasm,
        }
    }

    /// Echoes `on_state` input to output; `before_act` always returns `{"allow":true}`.
    fn echo_wasm() -> Vec<u8> {
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
                    (i32.store8 (i32.add (local.get $out_ptr) (i32.const 9)) (i32.const 116))
                    (i32.store8 (i32.add (local.get $out_ptr) (i32.const 10)) (i32.const 114))
                    (i32.store8 (i32.add (local.get $out_ptr) (i32.const 11)) (i32.const 117))
                    (i32.store8 (i32.add (local.get $out_ptr) (i32.const 12)) (i32.const 101))
                    (i32.store8 (i32.add (local.get $out_ptr) (i32.const 13)) (i32.const 125))
                    (i32.store (local.get $out_len_ptr) (i32.const 14))
                )
            )"#,
        )
        .unwrap()
    }

    /// `before_act` always returns `{"allow":false}`. No `on_state` export.
    fn block_wasm() -> Vec<u8> {
        wat::parse_str(
            r#"(module
                (memory (export "memory") 1)
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

    fn dummy_manifest_path() -> PathBuf {
        PathBuf::from("test-fixture-manifest.json")
    }

    /// `PluginHookConfig` has no `Debug` impl (it holds trait objects), so
    /// `Result::unwrap_err` can't be used directly on `build_plugin_hook_config`'s result.
    fn expect_err(result: Result<PluginHookConfig, PluginWiringError>) -> PluginWiringError {
        match result {
            Ok(_) => panic!("expected an error, got Ok"),
            Err(err) => err,
        }
    }

    #[test]
    fn wires_onstate_plugin_into_state_hooks() {
        let signing_key = SigningKey::from_bytes(&[11u8; 32]);
        let registry = key_registry_with(&signing_key, "k1");
        let package = signed_package(
            "onstate.plugin",
            vec![ExtensionPoint::OnState],
            vec![plugin_host::Capability::ReadState],
            echo_wasm(),
            &signing_key,
            "k1",
        );

        let config = build_plugin_hook_config(
            registry,
            vec![ConfiguredPlugin {
                manifest_path: dummy_manifest_path(),
                package,
            }],
        )
        .unwrap();

        assert_eq!(config.state_plugins.len(), 1);
        assert_eq!(config.policy_plugins.len(), 0);
        let (result, events) = run_state_hooks(
            r#"{"url":"https://example.com"}"#,
            config.state_plugins.iter().map(|p| p.as_ref()),
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&result).unwrap()["url"],
            "https://example.com"
        );
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            core_runtime::audit::AuditEvent::PluginStateTransform { success: true, .. }
        ));
    }

    #[test]
    fn wires_beforeact_plugin_into_policy_hooks_and_blocks() {
        let signing_key = SigningKey::from_bytes(&[12u8; 32]);
        let registry = key_registry_with(&signing_key, "k2");
        let package = signed_package(
            "block.plugin",
            vec![ExtensionPoint::BeforeAct],
            vec![],
            block_wasm(),
            &signing_key,
            "k2",
        );

        let config = build_plugin_hook_config(
            registry,
            vec![ConfiguredPlugin {
                manifest_path: dummy_manifest_path(),
                package,
            }],
        )
        .unwrap();

        assert_eq!(config.policy_plugins.len(), 1);
        let (outcome, _events) = run_policy_hooks(
            r#"{"action":"click"}"#,
            config.policy_plugins.iter().map(|p| p.as_ref()),
        );
        assert!(matches!(outcome, PolicyHookOutcome::Block { .. }));
    }

    #[test]
    fn rejects_unsigned_plugin_with_explicit_load_error() {
        let registry = KeyRegistry::default();
        let manifest = PluginManifest {
            plugin_id: "unsigned.plugin".to_string(),
            version: "0.1.0".to_string(),
            entry_points: vec![ExtensionPoint::OnState],
            capabilities: vec![plugin_host::Capability::ReadState],
            signature: None,
            sbom: sample_sbom(),
        };
        let package = PluginPackage {
            manifest,
            wasm_module: echo_wasm(),
        };

        let error = build_plugin_hook_config(
            registry,
            vec![ConfiguredPlugin {
                manifest_path: dummy_manifest_path(),
                package,
            }],
        );
        let error = expect_err(error);

        assert!(matches!(error, PluginWiringError::Load { .. }));
    }

    #[test]
    fn rejects_onstate_plugin_missing_read_state_capability() {
        let signing_key = SigningKey::from_bytes(&[13u8; 32]);
        let registry = key_registry_with(&signing_key, "k3");
        let package = signed_package(
            "no-capability.plugin",
            vec![ExtensionPoint::OnState],
            vec![], // ReadState intentionally omitted
            echo_wasm(),
            &signing_key,
            "k3",
        );

        let error = build_plugin_hook_config(
            registry,
            vec![ConfiguredPlugin {
                manifest_path: dummy_manifest_path(),
                package,
            }],
        );
        let error = expect_err(error);

        assert!(matches!(error, PluginWiringError::Adapter { .. }));
    }

    #[test]
    fn rejects_connector_only_plugin_as_unsupported() {
        let signing_key = SigningKey::from_bytes(&[14u8; 32]);
        let registry = key_registry_with(&signing_key, "k4");
        // `connector`-exporting wasm: export existence is all `PluginHost::load_plugin` checks.
        let wasm = wat::parse_str(
            r#"(module
                (memory (export "memory") 1)
                (func (export "connector")
                      (param i32) (param i32) (param i32) (param i32))
            )"#,
        )
        .unwrap();
        let package = signed_package(
            "connector-only.plugin",
            vec![ExtensionPoint::Connector],
            vec![plugin_host::Capability::NetworkOut],
            wasm,
            &signing_key,
            "k4",
        );

        let error = build_plugin_hook_config(
            registry,
            vec![ConfiguredPlugin {
                manifest_path: dummy_manifest_path(),
                package,
            }],
        );
        let error = expect_err(error);

        assert!(matches!(
            error,
            PluginWiringError::UnsupportedExtensionPoint { .. }
        ));
    }

    #[test]
    fn wires_mixed_onstate_and_connector_plugin_and_only_warns_about_connector() {
        // `on_state` handles both `on_state` and `connector` calls identically (echo); only the
        // declared entry points matter for this test, not what `connector` actually does.
        let signing_key = SigningKey::from_bytes(&[15u8; 32]);
        let registry = key_registry_with(&signing_key, "k5");
        let wasm = wat::parse_str(
            r#"(module
                (memory (export "memory") 1)
                (func (export "on_state")
                      (param i32) (param i32) (param $out_ptr i32) (param $out_len_ptr i32)
                    (i32.store (local.get $out_len_ptr) (i32.const 0)))
                (func (export "connector")
                      (param i32) (param i32) (param i32) (param i32))
            )"#,
        )
        .unwrap();
        let package = signed_package(
            "mixed.plugin",
            vec![ExtensionPoint::OnState, ExtensionPoint::Connector],
            vec![plugin_host::Capability::ReadState],
            wasm,
            &signing_key,
            "k5",
        );

        let config = build_plugin_hook_config(
            registry,
            vec![ConfiguredPlugin {
                manifest_path: dummy_manifest_path(),
                package,
            }],
        )
        .unwrap();

        assert_eq!(config.state_plugins.len(), 1);
    }

    #[test]
    fn rejects_duplicate_plugin_id_across_configured_entries() {
        let signing_key = SigningKey::from_bytes(&[16u8; 32]);
        let registry = key_registry_with(&signing_key, "k6");
        let package_a = signed_package(
            "duplicate.plugin",
            vec![ExtensionPoint::OnState],
            vec![plugin_host::Capability::ReadState],
            echo_wasm(),
            &signing_key,
            "k6",
        );
        let package_b = signed_package(
            "duplicate.plugin",
            vec![ExtensionPoint::OnState],
            vec![plugin_host::Capability::ReadState],
            echo_wasm(),
            &signing_key,
            "k6",
        );

        let error = build_plugin_hook_config(
            registry,
            vec![
                ConfiguredPlugin {
                    manifest_path: PathBuf::from("first-manifest.json"),
                    package: package_a,
                },
                ConfiguredPlugin {
                    manifest_path: PathBuf::from("second-manifest.json"),
                    package: package_b,
                },
            ],
        );
        let error = expect_err(error);

        assert!(matches!(error, PluginWiringError::DuplicatePluginId { .. }));
        assert!(error.to_string().contains("first-manifest.json"));
        assert!(error.to_string().contains("second-manifest.json"));
    }

    #[test]
    fn no_configured_plugins_produces_default_empty_config() {
        let config = build_plugin_hook_config(KeyRegistry::default(), vec![]).unwrap();
        assert_eq!(config.state_plugins.len(), 0);
        assert_eq!(config.policy_plugins.len(), 0);
    }
}
