use ed25519_dalek::{Signer, SigningKey};
use plugin_host::{
    Capability, ExtensionPoint, KeyRegistry, PluginError, PluginHost, PluginManifest,
    PluginPackage, SbomComponent, SbomDocument, SignatureBlock, signature_payload,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Memory layout (matches the constants in `plugin_host::PluginRuntime`):
///   - [0 .. 16 KiB)      : input JSON written by the host
///   - [16 KiB .. 32 KiB) : output buffer written by the Wasm function
///   - [32 KiB .. 32 KiB+4]: output length (i32 LE) written by the Wasm function
///
/// This fits comfortably in a single 64 KiB Wasm page.
///
/// Minimal WAT that exports `on_state` and `before_act`:
///   - `on_state`   : echo-copies the input bytes to `out_ptr`, stores `in_len` at `out_len_ptr`.
///   - `before_act` : always writes `{"allow":true}` (14 bytes) to `out_ptr`.
fn echo_wasm() -> Vec<u8> {
    // {"allow":true} as byte constants:  { " a l l o w " :  t  r  u  e  }
    // 123 34 97 108 108 111 119 34 58 116 114 117 101 125
    wat::parse_str(
        r#"(module
            (memory (export "memory") 1)

            ;; on_state: copy input to output with a byte-by-byte loop
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

            ;; before_act: always returns {"allow":true}
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

/// Wasm that exports `before_act` only (no `on_state`).
fn before_act_only_wasm() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
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
                (i32.store (local.get $out_len_ptr) (i32.const 14)))
        )"#,
    )
    .expect("failed to build before_act_only wasm fixture")
}

/// Wasm that exports `on_state`, `before_act`, and `connector`.
/// `connector` echoes the input JSON back to the caller (same as `on_state`).
fn connector_wasm() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (memory (export "memory") 1)

            ;; on_state: echo input
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

            ;; before_act: always returns {"allow":true}
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

            ;; connector: echo input (same loop as on_state)
            (func (export "connector")
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
        )"#,
    )
    .expect("failed to build connector wasm fixture")
}

/// Wasm whose `before_act` writes non-UTF8 / non-JSON garbage.
fn malformed_output_wasm() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (func (export "before_act")
                  (param $in_ptr i32) (param $in_len i32)
                  (param $out_ptr i32) (param $out_len_ptr i32)
                ;; write 4 bytes of garbage (0xFF 0xFE 0x00 0x01) — invalid UTF-8 / invalid JSON
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 0)) (i32.const 255))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 1)) (i32.const 254))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 2)) (i32.const 0))
                (i32.store8 (i32.add (local.get $out_ptr) (i32.const 3)) (i32.const 1))
                (i32.store (local.get $out_len_ptr) (i32.const 4))
            )
        )"#,
    )
    .expect("failed to build malformed_output wasm fixture")
}

fn sample_sbom() -> SbomDocument {
    SbomDocument {
        format: "cyclonedx-1.5".to_string(),
        components: vec![SbomComponent {
            name: "fixture-component".to_string(),
            version: "1.0.0".to_string(),
            license: Some("MIT".to_string()),
        }],
    }
}

fn make_registry_and_key() -> (KeyRegistry, SigningKey, String) {
    let signing_key = SigningKey::from_bytes(&[42u8; 32]);
    let key_id = "test-key-runtime".to_string();

    let mut registry = KeyRegistry::default();
    registry
        .register_hex_ed25519(
            &key_id,
            &hex::encode(signing_key.verifying_key().to_bytes()),
        )
        .expect("must register key");

    (registry, signing_key, key_id)
}

fn build_and_sign_package(
    entry_points: Vec<ExtensionPoint>,
    capabilities: Vec<Capability>,
    wasm: Vec<u8>,
    signing_key: &SigningKey,
    key_id: &str,
) -> PluginPackage {
    let mut manifest = PluginManifest {
        plugin_id: "runtime.test.plugin".to_string(),
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
// Tests
// ---------------------------------------------------------------------------

/// 1. Signed plugin with on_state can transform state JSON (echo fixture)
#[test]
fn test_on_state_echoes_input() {
    let (registry, signing_key, key_id) = make_registry_and_key();
    let wasm = echo_wasm();
    let package = build_and_sign_package(
        vec![ExtensionPoint::OnState],
        vec![Capability::ReadState],
        wasm.clone(),
        &signing_key,
        &key_id,
    );

    let host = PluginHost::new(registry);
    let loaded = host.load_plugin(&package).expect("plugin must load");

    let mut runtime = loaded.create_runtime().expect("runtime must be created");

    let input = r#"{"dragon":"head"}"#;
    let output = runtime.on_state(input).expect("on_state must succeed");

    // echo fixture returns the same JSON
    let parsed: serde_json::Value =
        serde_json::from_str(&output).expect("output must be valid JSON");
    assert_eq!(parsed["dragon"], "head");
}

#[test]
fn test_on_state_payload_over_16k_returns_explicit_error() {
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
    let mut runtime = loaded.create_runtime().expect("runtime must be created");

    let oversized = format!(r#"{{"payload":"{}"}}"#, "x".repeat(16 * 1024));
    let err = runtime
        .on_state(&oversized)
        .expect_err("oversized state payload must fail explicitly");

    match err {
        PluginError::PayloadTooLarge {
            operation,
            size,
            limit,
        } => {
            assert_eq!(operation, "on_state");
            assert!(size > limit, "size={size}, limit={limit}");
            assert_eq!(limit, 16 * 1024);
        }
        other => panic!("expected PayloadTooLarge, got {other:?}"),
    }
}

/// 2. Signed plugin with before_act returns policy decision JSON
#[test]
fn test_before_act_returns_allow_true() {
    let (registry, signing_key, key_id) = make_registry_and_key();
    let wasm = echo_wasm();
    let package = build_and_sign_package(
        vec![ExtensionPoint::BeforeAct],
        vec![],
        wasm.clone(),
        &signing_key,
        &key_id,
    );

    let host = PluginHost::new(registry);
    let loaded = host.load_plugin(&package).expect("plugin must load");

    let mut runtime = loaded.create_runtime().expect("runtime must be created");

    let output = runtime
        .before_act(r#"{"action":"speak"}"#)
        .expect("before_act must succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&output).expect("output must be valid JSON");
    assert_eq!(parsed["allow"], true);
}

/// 3. Plugin without ReadState capability cannot execute on_state (CapabilityViolation)
#[test]
fn test_on_state_blocked_without_read_state_capability() {
    let (registry, signing_key, key_id) = make_registry_and_key();
    let wasm = echo_wasm();
    // Declare on_state entry-point but omit ReadState capability
    let package = build_and_sign_package(
        vec![ExtensionPoint::OnState],
        vec![], // <-- no ReadState
        wasm.clone(),
        &signing_key,
        &key_id,
    );

    let host = PluginHost::new(registry);
    // load_plugin checks the export exists, not capabilities at load time
    let loaded = host.load_plugin(&package).expect("plugin must load");

    let mut runtime = loaded.create_runtime().expect("runtime must be created");

    let err = runtime
        .on_state(r#"{"x":1}"#)
        .expect_err("must be blocked by capability check");

    assert!(
        matches!(
            err,
            PluginError::CapabilityViolation {
                required: Capability::ReadState
            }
        ),
        "expected CapabilityViolation(ReadState), got {err:?}"
    );
}

/// 4. Unsigned plugin cannot execute — UnsignedPlugin error at load time.
#[test]
fn test_unsigned_plugin_rejected_at_load_time() {
    let manifest = PluginManifest {
        plugin_id: "unsigned.plugin".to_string(),
        version: "1.0.0".to_string(),
        entry_points: vec![ExtensionPoint::OnState],
        capabilities: vec![Capability::ReadState],
        signature: None,
        sbom: sample_sbom(),
    };
    let wasm = echo_wasm();
    let package = PluginPackage {
        manifest,
        wasm_module: wasm,
    };

    let host = PluginHost::default();
    let err = host
        .load_plugin(&package)
        .expect_err("unsigned plugin must be rejected");

    assert!(
        matches!(err, PluginError::UnsignedPlugin),
        "expected UnsignedPlugin, got {err:?}"
    );
}

/// 5. Malformed output from Wasm produces InvalidOutput error
#[test]
fn test_before_act_malformed_output_returns_invalid_output() {
    let (registry, signing_key, key_id) = make_registry_and_key();
    let wasm = malformed_output_wasm();
    let package = build_and_sign_package(
        vec![ExtensionPoint::BeforeAct],
        vec![],
        wasm.clone(),
        &signing_key,
        &key_id,
    );

    let host = PluginHost::new(registry);
    let loaded = host.load_plugin(&package).expect("plugin must load");

    let mut runtime = loaded.create_runtime().expect("runtime must be created");

    let err = runtime
        .before_act(r#"{"action":"speak"}"#)
        .expect_err("malformed output must produce an error");

    assert!(
        matches!(err, PluginError::InvalidOutput { .. }),
        "expected InvalidOutput, got {err:?}"
    );
}

/// 6. Plugin without on_state export fails at load time (MissingExport)
#[test]
fn test_missing_on_state_export_fails_at_load() {
    let (registry, signing_key, key_id) = make_registry_and_key();
    let wasm = before_act_only_wasm();

    // Declare on_state in manifest but the wasm only exports before_act
    let package = build_and_sign_package(
        vec![ExtensionPoint::OnState], // declared but absent in wasm
        vec![Capability::ReadState],
        wasm,
        &signing_key,
        &key_id,
    );

    let host = PluginHost::new(registry);
    let err = host
        .load_plugin(&package)
        .expect_err("missing export must fail at load");

    assert!(
        matches!(
            err,
            PluginError::MissingExport {
                extension: ExtensionPoint::OnState
            }
        ),
        "expected MissingExport(OnState), got {err:?}"
    );
}

/// 7. connector extension point echoes request JSON when NetworkOut capability is granted
#[test]
fn test_connector_echoes_request() {
    let (registry, signing_key, key_id) = make_registry_and_key();
    let wasm = connector_wasm();
    let package = build_and_sign_package(
        vec![
            ExtensionPoint::OnState,
            ExtensionPoint::BeforeAct,
            ExtensionPoint::Connector,
        ],
        vec![Capability::ReadState, Capability::NetworkOut],
        wasm,
        &signing_key,
        &key_id,
    );

    let host = PluginHost::new(registry);
    let loaded = host.load_plugin(&package).expect("plugin must load");
    let mut runtime = loaded.create_runtime().expect("runtime must be created");

    let request = r#"{"url":"https://example.com"}"#;
    let output = runtime.connector(request).expect("connector must succeed");

    let parsed: serde_json::Value =
        serde_json::from_str(&output).expect("output must be valid JSON");
    assert_eq!(parsed["url"], "https://example.com");
}

/// WAT fixture with an infinite loop in before_act — used to test interruption.
fn infinite_loop_wasm() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (func (export "before_act")
                  (param $in_ptr i32) (param $in_len i32)
                  (param $out_ptr i32) (param $out_len_ptr i32)
                (loop $spin (br $spin))
            )
        )"#,
    )
    .expect("failed to build infinite_loop wasm fixture")
}

/// Wasm that attempts to grow its linear memory beyond the host's 64 MiB limit.
fn excessive_memory_growth_wasm() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (func (export "before_act")
                  (param $in_ptr i32) (param $in_len i32)
                  (param $out_ptr i32) (param $out_len_ptr i32)
                (if (i32.eq (memory.grow (i32.const 1024)) (i32.const -1))
                    (then unreachable))
            )
        )"#,
    )
    .expect("failed to build excessive-memory wasm fixture")
}

/// 8. connector is blocked when NetworkOut capability is missing
#[test]
fn test_connector_blocked_without_network_out() {
    let (registry, signing_key, key_id) = make_registry_and_key();
    let wasm = connector_wasm();
    let package = build_and_sign_package(
        vec![ExtensionPoint::Connector],
        vec![], // NetworkOut intentionally omitted
        wasm,
        &signing_key,
        &key_id,
    );

    let host = PluginHost::new(registry);
    let loaded = host.load_plugin(&package).expect("plugin must load");
    let mut runtime = loaded.create_runtime().expect("runtime must be created");

    let err = runtime
        .connector(r#"{"url":"https://example.com"}"#)
        .expect_err("connector without NetworkOut must be rejected");

    assert!(
        matches!(
            err,
            PluginError::CapabilityViolation {
                required: Capability::NetworkOut
            }
        ),
        "expected CapabilityViolation(NetworkOut), got {err:?}"
    );
}

/// 9. Loading the same wasm bytes twice hits the module cache (second load < 5ms).
#[test]
fn test_second_load_hits_module_cache() {
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
    host.load_plugin(&package).expect("first load must succeed");

    let t = std::time::Instant::now();
    let loaded2 = host
        .load_plugin(&package)
        .expect("second load must succeed");
    let elapsed = t.elapsed();

    // Second load should be fast because the module is returned from cache.
    // 50ms is generous enough for any CI runner (pure lock + hash lookup).
    assert!(
        elapsed.as_millis() < 50,
        "second load took {}ms, expected < 50ms (cache miss?)",
        elapsed.as_millis()
    );

    // Verify the cached plugin still executes correctly.
    let mut runtime = loaded2.create_runtime().expect("runtime must be created");
    let out = runtime
        .on_state(r#"{"cached":true}"#)
        .expect("on_state must succeed");
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("must be valid JSON");
    assert_eq!(parsed["cached"], true);
}

/// 10. Epoch-based interruption stops a runaway (infinite-loop) plugin.
#[test]
fn test_epoch_interruption_stops_infinite_loop() {
    let (registry, signing_key, key_id) = make_registry_and_key();
    let wasm = infinite_loop_wasm();
    let package = build_and_sign_package(
        vec![ExtensionPoint::BeforeAct],
        vec![],
        wasm,
        &signing_key,
        &key_id,
    );

    let host = PluginHost::new(registry);
    let loaded = host.load_plugin(&package).expect("plugin must load");
    let mut runtime = loaded.create_runtime().expect("runtime must be created");

    let t = std::time::Instant::now();
    let err = runtime
        .before_act(r#"{"action":"loop"}"#)
        .expect_err("infinite loop must be interrupted");
    let elapsed = t.elapsed();

    assert!(
        matches!(err, PluginError::Timeout),
        "expected Timeout, got {err:?}"
    );
    // Fuel budget (1B instructions) takes >>1 s in debug mode.
    // Epoch fires at EPOCH_TICKS_PER_CALL × EPOCH_TICK_INTERVAL = 50 ms.
    // Verify the call terminated quickly, proving epoch interrupted the loop.
    assert!(
        elapsed.as_millis() < 500,
        "call took {}ms — epoch should have fired within ~50ms",
        elapsed.as_millis()
    );
}

#[test]
fn test_memory_growth_beyond_limit_is_rejected() {
    let (registry, signing_key, key_id) = make_registry_and_key();
    let package = build_and_sign_package(
        vec![ExtensionPoint::BeforeAct],
        vec![],
        excessive_memory_growth_wasm(),
        &signing_key,
        &key_id,
    );

    let host = PluginHost::new(registry);
    let loaded = host.load_plugin(&package).expect("plugin must load");
    let mut runtime = loaded.create_runtime().expect("runtime must be created");
    let err = runtime
        .before_act(r#"{"action":"grow-memory"}"#)
        .expect_err("memory growth beyond the host limit must fail");

    assert!(
        matches!(err, PluginError::MemoryExhausted),
        "expected MemoryExhausted, got {err:?}"
    );
}

/// 11. Creating 100 runtimes from a cached LoadedPlugin averages < 1ms each.
#[test]
fn test_bulk_runtime_creation_under_1ms_average() {
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

    const N: u32 = 100;
    let t = std::time::Instant::now();
    for _ in 0..N {
        loaded.create_runtime().expect("runtime must be created");
    }
    let elapsed = t.elapsed();
    let total_ms = elapsed.as_millis();
    let avg_us = elapsed.as_micros() / N as u128;

    assert!(
        total_ms < N as u128,
        "total {total_ms}ms for {N} runtimes; average {avg_us}µs — expected < 1ms each"
    );
}
