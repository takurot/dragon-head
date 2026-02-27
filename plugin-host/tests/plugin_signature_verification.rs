use ed25519_dalek::{Signer, SigningKey};
use plugin_host::{
    Capability, ExtensionPoint, KeyRegistry, PluginError, PluginHost, PluginManifest,
    PluginPackage, SbomComponent, SbomDocument, SignatureBlock, signature_payload,
};

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
    let payload = signature_payload(manifest, wasm).expect("signature payload must be buildable");
    let signature = signing_key.sign(&payload);

    SignatureBlock {
        key_id: key_id.to_string(),
        signature_hex: hex::encode(signature.to_bytes()),
    }
}

#[test]
fn test_rejects_unsigned_plugin() {
    let host = PluginHost::default();
    let package = PluginPackage {
        manifest: sample_manifest(vec![Capability::ReadState]),
        wasm_module: sample_wasm_module(),
    };

    let err = host
        .load_plugin(&package)
        .expect_err("unsigned plugin must fail");
    assert!(matches!(err, PluginError::UnsignedPlugin));
}

#[test]
fn test_accepts_signed_plugin_and_blocks_capability_violation() {
    let signing_key = SigningKey::from_bytes(&[7u8; 32]);
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
    let plugin = host
        .load_plugin(&package)
        .expect("signed plugin should be accepted");

    plugin
        .authorize_extension(ExtensionPoint::OnState)
        .expect("on_state should be allowed with read_state capability");

    let err = plugin
        .authorize_extension(ExtensionPoint::Connector)
        .expect_err("connector must be blocked without network_out capability");
    assert!(matches!(
        err,
        PluginError::CapabilityViolation {
            required: Capability::NetworkOut
        }
    ));
}
