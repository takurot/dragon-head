use ed25519_dalek::{Signer, SigningKey};
use plugin_host::{
    Capability, DomainPackBundle, DomainPackManifest, DomainPackMarketplaceMetadata,
    DomainPackSkill, ExtensionPoint, KeyRegistry, MarketplaceDependency, PluginError, PluginHost,
    PluginManifest, PluginPackage, RuntimeCompatibility, SbomComponent, SbomDocument,
    SignatureBlock, domain_pack_signature_payload, signature_payload,
};
use serde_json::json;

fn sample_wasm_module() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (func (export "on_state"))
            (func (export "before_act"))
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

fn sample_plugin_manifest() -> PluginManifest {
    PluginManifest {
        plugin_id: "acme.checkout".to_string(),
        version: "1.3.0".to_string(),
        entry_points: vec![ExtensionPoint::OnState, ExtensionPoint::BeforeAct],
        capabilities: vec![Capability::ReadState],
        signature: None,
        sbom: sample_sbom(),
    }
}

fn sign_plugin_manifest(
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

fn sign_domain_pack(
    bundle: &DomainPackBundle,
    signing_key: &SigningKey,
    key_id: &str,
) -> SignatureBlock {
    let payload = domain_pack_signature_payload(bundle)
        .expect("domain pack signature payload must be buildable");
    let signature = signing_key.sign(&payload);

    SignatureBlock {
        key_id: key_id.to_string(),
        signature_hex: hex::encode(signature.to_bytes()),
    }
}

fn sample_bundle(signing_key: &SigningKey, key_id: &str) -> DomainPackBundle {
    let wasm_module = sample_wasm_module();
    let mut plugin_manifest = sample_plugin_manifest();
    plugin_manifest.signature = Some(sign_plugin_manifest(
        &plugin_manifest,
        &wasm_module,
        signing_key,
        key_id,
    ));

    let plugin = PluginPackage {
        manifest: plugin_manifest,
        wasm_module,
    };

    let marketplace = DomainPackMarketplaceMetadata {
        publisher_id: "acme-inc".to_string(),
        runtime_compatibility: RuntimeCompatibility {
            runtime_api: "2.1".to_string(),
            min_runtime_version: "0.16.0".to_string(),
            max_runtime_version: Some("0.20.0".to_string()),
        },
        dependencies: vec![MarketplaceDependency {
            package: "skills-engine.checkout".to_string(),
            version_req: "^1.0".to_string(),
        }],
        signature: None,
    };

    let manifest = DomainPackManifest {
        pack_id: "acme.checkout.pack".to_string(),
        version: "2026.03.0".to_string(),
        plugin_id: "acme.checkout".to_string(),
        skill_ids: vec!["checkout.submit".to_string()],
        marketplace,
    };

    let skills = vec![DomainPackSkill {
        skill_id: "checkout.submit".to_string(),
        version: "1.0.0".to_string(),
        definition: json!({
            "schema_version": 1,
            "name": "checkout.submit",
            "steps": [
                {
                    "type": "verify",
                    "target": "id:42",
                    "expected": "Purchase"
                },
                {
                    "type": "act",
                    "action": "click",
                    "target": "id:42"
                }
            ]
        }),
    }];

    let mut bundle = DomainPackBundle {
        manifest,
        plugin,
        skills,
    };

    let signature = sign_domain_pack(&bundle, signing_key, key_id);
    bundle.manifest.marketplace.signature = Some(signature);
    bundle
}

#[test]
fn test_load_domain_pack_accepts_signed_compatible_bundle() {
    let signing_key = SigningKey::from_bytes(&[11u8; 32]);
    let key_id = "marketplace-key";

    let mut registry = KeyRegistry::default();
    registry
        .register_hex_ed25519(key_id, &hex::encode(signing_key.verifying_key().to_bytes()))
        .expect("must register key");

    let host = PluginHost::new(registry);
    let bundle = sample_bundle(&signing_key, key_id);

    let loaded = host
        .load_domain_pack(&bundle, "0.18.2")
        .expect("signed compatible bundle should load");

    assert_eq!(loaded.manifest().pack_id, "acme.checkout.pack");
    assert_eq!(loaded.skill_ids(), vec!["checkout.submit"]);
}

#[test]
fn test_load_domain_pack_rejects_incompatible_runtime_version() {
    let signing_key = SigningKey::from_bytes(&[13u8; 32]);
    let key_id = "marketplace-key";

    let mut registry = KeyRegistry::default();
    registry
        .register_hex_ed25519(key_id, &hex::encode(signing_key.verifying_key().to_bytes()))
        .expect("must register key");

    let host = PluginHost::new(registry);
    let bundle = sample_bundle(&signing_key, key_id);

    let err = host
        .load_domain_pack(&bundle, "0.24.0")
        .expect_err("runtime outside compatibility range must fail");

    assert!(matches!(
        err,
        PluginError::IncompatibleRuntimeVersion { .. }
    ));
}
