use ed25519_dalek::{Signer, SigningKey};
use plugin_host::{
    Capability, ExtensionPoint, KeyRegistry, PluginError, PluginHost, PluginManifest,
    PluginPackage, SbomComponent, SbomDocument, SignatureBlock, signature_payload,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use test_bench_support::{EvaluationBench, EvaluationMode};

#[test]
fn test_plugin_host_comprehensive_evaluation_suite() -> anyhow::Result<()> {
    let mut bench = EvaluationBench::new(
        "plugin-host",
        "comprehensive_evaluation",
        EvaluationMode::from_env(),
    );

    bench.run_scenario(
        "unsigned_plugin_rejected",
        "signature",
        scenario_unsigned_plugin_rejected,
    );
    bench.run_scenario(
        "signed_plugin_capability_enforcement",
        "capability",
        scenario_signed_plugin_capability_enforcement,
    );
    // Adversarial signature-rejection coverage (issue #284) — a suite named
    // "comprehensive" previously only exercised the valid-signature happy
    // path plus the no-signature-at-all case; none of these tamper/mismatch
    // paths through `verify_signature` had a test forcing them to fire.
    bench.run_scenario(
        "tampered_wasm_rejected",
        "signature",
        scenario_tampered_wasm_rejected,
    );
    bench.run_scenario(
        "tampered_manifest_rejected",
        "signature",
        scenario_tampered_manifest_rejected,
    );
    bench.run_scenario(
        "unknown_signing_key_rejected",
        "signature",
        scenario_unknown_signing_key_rejected,
    );
    bench.run_scenario(
        "wrong_key_signature_rejected",
        "signature",
        scenario_wrong_key_signature_rejected,
    );

    bench.write_if_configured()?;
    bench.assert_required_scenarios(&[
        "unsigned_plugin_rejected",
        "signed_plugin_capability_enforcement",
        "tampered_wasm_rejected",
        "tampered_manifest_rejected",
        "unknown_signing_key_rejected",
        "wrong_key_signature_rejected",
    ])?;
    bench.assert_all_passed()?;
    Ok(())
}

fn scenario_unsigned_plugin_rejected() -> anyhow::Result<Value> {
    let host = PluginHost::default();
    let package = PluginPackage {
        manifest: sample_manifest(vec![Capability::ReadState]),
        wasm_module: sample_wasm_module(),
    };

    let err = host
        .load_plugin(&package)
        .expect_err("unsigned plugin must fail");
    assert!(matches!(err, PluginError::UnsignedPlugin));

    Ok(json!({
        "error": "UnsignedPlugin",
    }))
}

fn scenario_signed_plugin_capability_enforcement() -> anyhow::Result<Value> {
    let signing_key = fixture_signing_key("signed_plugin_capability_enforcement");
    let key_id = "fixture-key";

    let mut registry = KeyRegistry::default();
    registry
        .register_hex_ed25519(key_id, &hex::encode(signing_key.verifying_key().to_bytes()))
        .expect("must register key");

    let wasm_module = sample_wasm_module();
    let mut manifest = sample_manifest(vec![Capability::ReadState]);
    manifest.signature = Some(sign_manifest(&manifest, &wasm_module, &signing_key, key_id));

    let package = PluginPackage {
        manifest,
        wasm_module,
    };

    let host = PluginHost::new(registry);
    let plugin = host.load_plugin(&package)?;
    plugin.authorize_extension(ExtensionPoint::OnState)?;

    // BeforeAct is declared in the manifest's entry_points (see
    // sample_manifest) and requires no capability, but was never actually
    // exercised by any scenario — issue #284.
    plugin.authorize_extension(ExtensionPoint::BeforeAct)?;

    let err = plugin
        .authorize_extension(ExtensionPoint::Connector)
        .expect_err("connector must be blocked without network_out capability");
    assert!(matches!(
        err,
        PluginError::CapabilityViolation {
            required: Capability::NetworkOut
        }
    ));

    Ok(json!({
        "on_state": "allowed",
        "before_act": "allowed",
        "connector": "blocked",
    }))
}

/// A signature verified against the wasm bytes it was signed over, but
/// loaded with a *different* wasm module — `signature_payload` folds the
/// wasm's SHA-256 into what's signed, so swapping the module after signing
/// must be rejected exactly like any other tamper (issue #284).
fn scenario_tampered_wasm_rejected() -> anyhow::Result<Value> {
    let signing_key = fixture_signing_key("tampered_wasm_rejected");
    let key_id = "fixture-key";

    let mut registry = KeyRegistry::default();
    registry
        .register_hex_ed25519(key_id, &hex::encode(signing_key.verifying_key().to_bytes()))
        .expect("must register key");

    let signed_wasm = sample_wasm_module();
    let mut manifest = sample_manifest(vec![Capability::ReadState]);
    manifest.signature = Some(sign_manifest(&manifest, &signed_wasm, &signing_key, key_id));

    // A different, but still validly-exported, wasm module substituted in
    // after signing — the signature was computed over `signed_wasm`, not
    // this one.
    let tampered_wasm = wat::parse_str(
        r#"(module
            (func (export "on_state"))
            (func (export "before_act"))
            (func (export "connector"))
            (func (export "extra_export"))
        )"#,
    )
    .expect("failed to build tampered wasm fixture");

    let package = PluginPackage {
        manifest,
        wasm_module: tampered_wasm,
    };

    let host = PluginHost::new(registry);
    let err = host
        .load_plugin(&package)
        .expect_err("plugin with a swapped wasm module must fail signature verification");
    assert!(matches!(err, PluginError::InvalidSignature));

    Ok(json!({ "error": "InvalidSignature", "tamper": "wasm_module" }))
}

/// Same idea as `scenario_tampered_wasm_rejected`, but the manifest is
/// mutated after signing instead of the wasm bytes — e.g. an attacker
/// escalating a signed manifest's capabilities post-hoc (issue #284).
fn scenario_tampered_manifest_rejected() -> anyhow::Result<Value> {
    let signing_key = fixture_signing_key("tampered_manifest_rejected");
    let key_id = "fixture-key";

    let mut registry = KeyRegistry::default();
    registry
        .register_hex_ed25519(key_id, &hex::encode(signing_key.verifying_key().to_bytes()))
        .expect("must register key");

    let wasm_module = sample_wasm_module();
    let mut manifest = sample_manifest(vec![Capability::ReadState]);
    manifest.signature = Some(sign_manifest(&manifest, &wasm_module, &signing_key, key_id));

    // Escalate capabilities after the signature was computed.
    manifest.capabilities.push(Capability::NetworkOut);

    let package = PluginPackage {
        manifest,
        wasm_module,
    };

    let host = PluginHost::new(registry);
    let err = host
        .load_plugin(&package)
        .expect_err("plugin with a post-signature manifest edit must fail verification");
    assert!(matches!(err, PluginError::InvalidSignature));

    Ok(json!({ "error": "InvalidSignature", "tamper": "manifest_capabilities" }))
}

/// The signature references a `key_id` that was never registered with the
/// host — must be rejected distinctly from an invalid/mismatched signature
/// (issue #284).
fn scenario_unknown_signing_key_rejected() -> anyhow::Result<Value> {
    let signing_key = fixture_signing_key("unknown_signing_key_rejected");
    let key_id = "never-registered-key";

    let wasm_module = sample_wasm_module();
    let mut manifest = sample_manifest(vec![Capability::ReadState]);
    manifest.signature = Some(sign_manifest(&manifest, &wasm_module, &signing_key, key_id));

    let package = PluginPackage {
        manifest,
        wasm_module,
    };

    // Empty registry: `key_id` above was never registered.
    let host = PluginHost::new(KeyRegistry::default());
    let err = host
        .load_plugin(&package)
        .expect_err("plugin signed with an unregistered key must be rejected");
    assert!(matches!(
        err,
        PluginError::UnknownSigningKey { key_id: ref k } if k == key_id
    ));

    Ok(json!({ "error": "UnknownSigningKey" }))
}

/// A validly-registered `key_id`, but the manifest was actually signed by a
/// *different* key than the one registered under that id — the registry
/// lookup succeeds, so this must fail at signature verification, not key
/// lookup (issue #284).
fn scenario_wrong_key_signature_rejected() -> anyhow::Result<Value> {
    let registered_key = fixture_signing_key("wrong_key_signature_rejected_registered");
    let attacker_key = fixture_signing_key("wrong_key_signature_rejected_attacker");
    let key_id = "fixture-key";

    let mut registry = KeyRegistry::default();
    registry
        .register_hex_ed25519(
            key_id,
            &hex::encode(registered_key.verifying_key().to_bytes()),
        )
        .expect("must register key");

    let wasm_module = sample_wasm_module();
    let mut manifest = sample_manifest(vec![Capability::ReadState]);
    // Signed with `attacker_key`, but claims `key_id` — which the registry
    // maps to `registered_key`'s public key.
    manifest.signature = Some(sign_manifest(
        &manifest,
        &wasm_module,
        &attacker_key,
        key_id,
    ));

    let package = PluginPackage {
        manifest,
        wasm_module,
    };

    let host = PluginHost::new(registry);
    let err = host
        .load_plugin(&package)
        .expect_err("signature from a key other than the one registered for key_id must fail");
    assert!(matches!(err, PluginError::InvalidSignature));

    Ok(json!({ "error": "InvalidSignature", "tamper": "wrong_signing_key" }))
}

/// Derives a deterministic-but-distinct 32-byte Ed25519 seed per fixture
/// purpose from its label, instead of a literal repeated-byte array
/// (`[7u8; 32]`) that risks being blindly copy-pasted into an unrelated
/// test and silently reused as the "same" key (issue #284).
fn fixture_signing_key(label: &str) -> SigningKey {
    let seed: [u8; 32] = Sha256::digest(label.as_bytes()).into();
    SigningKey::from_bytes(&seed)
}

fn sample_wasm_module() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (func (export "on_state"))
            (func (export "before_act"))
            (func (export "connector"))
        )"#,
    )
    .expect("failed to build wasm fixture")
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

fn sample_manifest(capabilities: Vec<Capability>) -> PluginManifest {
    PluginManifest {
        plugin_id: "fixture.plugin".to_string(),
        version: "0.1.0".to_string(),
        entry_points: vec![
            ExtensionPoint::OnState,
            ExtensionPoint::BeforeAct,
            ExtensionPoint::Connector,
        ],
        capabilities,
        signature: None,
        sbom: sample_sbom(),
    }
}

fn sign_manifest(
    manifest: &PluginManifest,
    wasm: &[u8],
    signing_key: &SigningKey,
    key_id: &str,
) -> SignatureBlock {
    // Preserve the real underlying error rather than an opaque helper panic
    // (issue #284).
    let payload = signature_payload(manifest, wasm)
        .unwrap_or_else(|err| panic!("signature payload must be buildable: {err}"));
    let signature = signing_key.sign(&payload);

    SignatureBlock {
        key_id: key_id.to_string(),
        signature_hex: hex::encode(signature.to_bytes()),
    }
}
