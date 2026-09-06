use ed25519_dalek::{Signer, SigningKey};
use marketplace::{
    DomainPack, MarketplaceMetadata, UsageEvent, calculate_revenue_share,
    domain_pack_signature_payload, encode_domain_pack_signature, verify_domain_pack,
};
use plugin_host::{
    Capability, ExtensionPoint, PluginManifest, PluginPackage, SbomComponent, SbomDocument,
    SignatureBlock,
};
use sha2::{Digest, Sha256};
use skills_engine::{LocateStep, SkillDefinition, SkillStep, StepControl};

fn sample_pack() -> DomainPack {
    DomainPack {
        metadata: MarketplaceMetadata {
            pack_id: "com.example.testpack".to_string(),
            author: "Test Author".to_string(),
            version: "1.0.0".to_string(),
            compatible_version: "2.1".to_string(),
            dependencies: vec!["alpha@1".to_string(), "beta@2".to_string()],
            signature: None,
        },
        plugin: Some(PluginPackage {
            manifest: PluginManifest {
                plugin_id: "com.example.plugin".to_string(),
                version: "1.0.0".to_string(),
                entry_points: vec![ExtensionPoint::OnState],
                capabilities: vec![Capability::ReadState],
                signature: None,
                sbom: SbomDocument {
                    format: "cyclonedx".to_string(),
                    components: vec![SbomComponent {
                        name: "sample".to_string(),
                        version: "1.0.0".to_string(),
                        license: Some("MIT".to_string()),
                    }],
                },
            },
            wasm_module: b"\0asm\x01\0\0\0".to_vec(),
        }),
        skills: vec![SkillDefinition {
            schema_version: 1,
            name: "find_checkout".to_string(),
            steps: vec![SkillStep::Locate(LocateStep {
                id: Some("find".to_string()),
                query: "checkout".to_string(),
                control: StepControl {
                    max_retries: 2,
                    on_success: None,
                    on_failure: Some("handoff".to_string()),
                },
            })],
        }],
    }
}

fn signing_key() -> SigningKey {
    SigningKey::from_bytes(&[7_u8; 32])
}

fn sign_pack(pack: &mut DomainPack) {
    pack.metadata.signature = None;
    let payload = domain_pack_signature_payload(pack).unwrap();
    pack.metadata.signature = Some(encode_domain_pack_signature(&signing_key().sign(&payload)));
}

fn assert_tamper_rejected(mut pack: DomainPack, mutate: impl FnOnce(&mut DomainPack)) {
    sign_pack(&mut pack);
    mutate(&mut pack);
    let pubkey = hex::encode(signing_key().verifying_key().to_bytes());
    assert!(verify_domain_pack(&pack, &pubkey).is_err());
}

#[test]
fn signed_domain_pack_verifies() {
    let mut pack = sample_pack();
    sign_pack(&mut pack);
    let pubkey_hex = hex::encode(signing_key().verifying_key().to_bytes());
    assert!(verify_domain_pack(&pack, &pubkey_hex).is_ok());

    let invalid_pubkey = "00".repeat(32);
    assert!(verify_domain_pack(&pack, &invalid_pubkey).is_err());
}

#[test]
fn legacy_and_unknown_signature_versions_are_rejected_without_fallback() {
    let mut pack = sample_pack();
    let payload = domain_pack_signature_payload(&pack).unwrap();
    let signature_hex = hex::encode(signing_key().sign(&payload).to_bytes());
    let pubkey_hex = hex::encode(signing_key().verifying_key().to_bytes());

    pack.metadata.signature = Some(signature_hex.clone());
    assert!(matches!(
        verify_domain_pack(&pack, &pubkey_hex),
        Err(marketplace::MarketplaceError::UnsupportedSignatureVersion(version)) if version == "legacy"
    ));

    pack.metadata.signature = Some(format!("v2:{signature_hex}"));
    assert!(matches!(
        verify_domain_pack(&pack, &pubkey_hex),
        Err(marketplace::MarketplaceError::UnsupportedSignatureVersion(version)) if version == "v2"
    ));
}

#[test]
fn plugin_wasm_and_manifest_tampering_is_rejected() {
    assert_tamper_rejected(sample_pack(), |pack| {
        pack.plugin.as_mut().unwrap().wasm_module[1] ^= 1;
    });
    assert_tamper_rejected(sample_pack(), |pack| {
        pack.plugin
            .as_mut()
            .unwrap()
            .manifest
            .capabilities
            .push(Capability::NetworkOut);
    });
    assert_tamper_rejected(sample_pack(), |pack| {
        pack.plugin.as_mut().unwrap().manifest.sbom.format = "tampered".to_string();
    });
    assert_tamper_rejected(sample_pack(), |pack| {
        pack.plugin.as_mut().unwrap().manifest.entry_points[0] = ExtensionPoint::BeforeAct;
    });
    assert_tamper_rejected(sample_pack(), |pack| {
        pack.plugin.as_mut().unwrap().manifest.signature = Some(SignatureBlock {
            key_id: "plugin-key".to_string(),
            signature_hex: "deadbeef".to_string(),
        });
    });
}

#[test]
fn nested_skill_tampering_is_rejected() {
    assert_tamper_rejected(sample_pack(), |pack| {
        let SkillStep::Locate(step) = &mut pack.skills[0].steps[0] else {
            unreachable!()
        };
        step.query = "steal credentials".to_string();
    });
    assert_tamper_rejected(sample_pack(), |pack| {
        let SkillStep::Locate(step) = &mut pack.skills[0].steps[0] else {
            unreachable!()
        };
        step.control.max_retries += 1;
    });
    assert_tamper_rejected(sample_pack(), |pack| {
        pack.skills[0].schema_version += 1;
    });
    assert_tamper_rejected(sample_pack(), |pack| {
        pack.skills[0].name = "tampered".to_string();
    });
    assert_tamper_rejected(sample_pack(), |pack| {
        let SkillStep::Locate(step) = &mut pack.skills[0].steps[0] else {
            unreachable!()
        };
        step.id = Some("tampered".to_string());
    });
}

#[test]
fn compatibility_dependencies_and_collection_order_are_signed() {
    assert_tamper_rejected(sample_pack(), |pack| {
        pack.metadata.pack_id = "com.attacker.pack".to_string();
    });
    assert_tamper_rejected(sample_pack(), |pack| {
        pack.metadata.author = "Attacker".to_string();
    });
    assert_tamper_rejected(sample_pack(), |pack| {
        pack.metadata.version = "9.9.9".to_string();
    });
    assert_tamper_rejected(sample_pack(), |pack| {
        pack.metadata.compatible_version = "99".to_string();
    });
    assert_tamper_rejected(sample_pack(), |pack| {
        pack.metadata.dependencies.swap(0, 1);
    });
    assert_tamper_rejected(sample_pack(), |pack| {
        pack.metadata.dependencies.push("gamma@3".to_string());
    });
    assert_tamper_rejected(sample_pack(), |pack| {
        pack.metadata.dependencies.remove(0);
    });
    assert_tamper_rejected(sample_pack(), |pack| {
        pack.metadata.dependencies[0] = "replacement@9".to_string();
    });
    assert_tamper_rejected(sample_pack(), |pack| {
        pack.plugin = None;
    });
    assert_tamper_rejected(sample_pack(), |pack| {
        pack.skills.clear();
    });
    let mut two_skills = sample_pack();
    let mut second_skill = two_skills.skills[0].clone();
    second_skill.name = "second".to_string();
    two_skills.skills.push(second_skill);
    assert_tamper_rejected(two_skills, |pack| pack.skills.swap(0, 1));
}

#[test]
fn signature_payload_is_deterministic_versioned_and_excludes_signature() {
    let pack = sample_pack();
    let first = domain_pack_signature_payload(&pack).unwrap();
    let second = domain_pack_signature_payload(&sample_pack()).unwrap();
    assert_eq!(first, second);
    assert!(
        String::from_utf8_lossy(&first).contains("dragon-head.marketplace.domain-pack-signature")
    );

    let mut signed = pack;
    signed.metadata.signature = Some("ignored".to_string());
    assert_eq!(first, domain_pack_signature_payload(&signed).unwrap());

    assert_eq!(
        hex::encode(Sha256::digest(&first)),
        "58016ba37767e0c3075934b47815b8f88f8d5cb25f5e974972369ad3dc6e9064"
    );
}

#[test]
fn test_revenue_share_calculation() {
    let event = UsageEvent {
        pack_id: "com.example.testpack".to_string(),
        event_type: "skill_run".to_string(),
        count: 100,
    };

    let revenue = calculate_revenue_share(&event);
    assert_eq!(revenue, 2.0); // 100 * 0.02 = 2.0

    let event_state = UsageEvent {
        pack_id: "com.example.testpack".to_string(),
        event_type: "state_generation".to_string(),
        count: 100,
    };

    let revenue_state = calculate_revenue_share(&event_state);
    assert_eq!(revenue_state, 0.5); // 100 * 0.005 = 0.5

    let event_action = UsageEvent {
        pack_id: "com.example.testpack".to_string(),
        event_type: "action_execution".to_string(),
        count: 100,
    };

    let revenue_action = calculate_revenue_share(&event_action);
    assert_eq!(revenue_action, 1.0); // 100 * 0.01 = 1.0

    // Unrecognized event types must still be billed, not silently free —
    // this default branch is the fallback for any event_type the pricing
    // table doesn't explicitly know about (e.g. a marketplace event kind
    // added later without updating calculate_revenue_share).
    let event_unknown = UsageEvent {
        pack_id: "com.example.testpack".to_string(),
        event_type: "some_future_event_type".to_string(),
        count: 100,
    };

    let revenue_unknown = calculate_revenue_share(&event_unknown);
    assert_eq!(revenue_unknown, 0.1); // 100 * 0.001 = 0.1
}
