use ed25519_dalek::{Signer, SigningKey};
use marketplace::{
    DomainPack, MarketplaceError, MarketplaceMetadata, RUNTIME_COMPATIBLE_VERSION, UsageEvent,
    build_signature_payload, calculate_revenue_share, check_compatibility, verify_domain_pack,
};
use rand_core::OsRng;

fn make_signed_pack(signing_key: &SigningKey) -> DomainPack {
    let metadata = MarketplaceMetadata {
        pack_id: "com.example.testpack".to_string(),
        author: "Test Author".to_string(),
        version: "1.0.0".to_string(),
        compatible_version: RUNTIME_COMPATIBLE_VERSION.to_string(),
        dependencies: vec!["dep-a".to_string(), "dep-b".to_string()],
        signature: None, // filled below
    };

    let payload = build_signature_payload(&metadata);
    let signature = signing_key.sign(&payload);

    DomainPack {
        metadata: MarketplaceMetadata {
            signature: Some(hex::encode(signature.to_bytes())),
            ..metadata
        },
        plugin: None,
        skills: vec![],
    }
}

// ── Signature Verification ───────────────────────────────────────────────

#[test]
fn test_signature_verification_valid() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let pubkey_hex = hex::encode(signing_key.verifying_key().to_bytes());
    let pack = make_signed_pack(&signing_key);

    assert!(verify_domain_pack(&pack, &pubkey_hex).is_ok());
}

#[test]
fn test_signature_verification_invalid_pubkey() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let pack = make_signed_pack(&signing_key);

    let invalid_pubkey = "00".repeat(32);
    let result = verify_domain_pack(&pack, &invalid_pubkey);
    assert!(result.is_err());
}

#[test]
fn test_unsigned_pack_rejected() {
    let metadata = MarketplaceMetadata {
        pack_id: "com.example.unsigned".to_string(),
        author: "Author".to_string(),
        version: "1.0.0".to_string(),
        compatible_version: RUNTIME_COMPATIBLE_VERSION.to_string(),
        dependencies: vec![],
        signature: None,
    };
    let pack = DomainPack {
        metadata,
        plugin: None,
        skills: vec![],
    };

    let result = verify_domain_pack(&pack, &"aa".repeat(32));
    assert_eq!(result, Err(MarketplaceError::UnsignedPack));
}

#[test]
fn test_tampered_metadata_rejected() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let pubkey_hex = hex::encode(signing_key.verifying_key().to_bytes());
    let mut pack = make_signed_pack(&signing_key);

    // Tamper with the compatible_version; signature should now be invalid
    pack.metadata.compatible_version = "999.0".to_string();

    let result = verify_domain_pack(&pack, &pubkey_hex);
    assert_eq!(result, Err(MarketplaceError::InvalidSignature));
}

#[test]
fn test_tampered_dependencies_rejected() {
    let signing_key = SigningKey::generate(&mut OsRng);
    let pubkey_hex = hex::encode(signing_key.verifying_key().to_bytes());
    let mut pack = make_signed_pack(&signing_key);

    // Tamper with dependency list
    pack.metadata.dependencies.push("malicious-dep".to_string());

    let result = verify_domain_pack(&pack, &pubkey_hex);
    assert_eq!(result, Err(MarketplaceError::InvalidSignature));
}

// ── Revenue Share ────────────────────────────────────────────────────────

#[test]
fn test_revenue_share_all_event_types() {
    let cases = [
        ("state_generation", 100, 0.5),
        ("action_execution", 100, 1.0),
        ("skill_run", 100, 2.0),
        ("unknown_event", 100, 0.1), // fallback rate
    ];

    for (event_type, count, expected) in cases {
        let event = UsageEvent {
            pack_id: "test".to_string(),
            event_type: event_type.to_string(),
            count,
        };
        let revenue = calculate_revenue_share(&event);
        assert!(
            (revenue - expected).abs() < f64::EPSILON,
            "event_type={event_type}: expected {expected}, got {revenue}"
        );
    }
}

#[test]
fn test_revenue_share_zero_count() {
    let event = UsageEvent {
        pack_id: "test".to_string(),
        event_type: "skill_run".to_string(),
        count: 0,
    };
    assert_eq!(calculate_revenue_share(&event), 0.0);
}

// ── Compatibility Check ──────────────────────────────────────────────────

#[test]
fn test_compatible_version_accepted() {
    let pack = DomainPack {
        metadata: MarketplaceMetadata {
            pack_id: "test".to_string(),
            author: "Author".to_string(),
            version: "1.0.0".to_string(),
            compatible_version: RUNTIME_COMPATIBLE_VERSION.to_string(),
            dependencies: vec![],
            signature: None,
        },
        plugin: None,
        skills: vec![],
    };
    assert!(check_compatibility(&pack).is_ok());
}

#[test]
fn test_incompatible_version_rejected() {
    let pack = DomainPack {
        metadata: MarketplaceMetadata {
            pack_id: "test".to_string(),
            author: "Author".to_string(),
            version: "1.0.0".to_string(),
            compatible_version: "999.0".to_string(),
            dependencies: vec![],
            signature: None,
        },
        plugin: None,
        skills: vec![],
    };
    let result = check_compatibility(&pack);
    assert_eq!(
        result,
        Err(MarketplaceError::IncompatibleVersion {
            pack: "999.0".to_string(),
            runtime: RUNTIME_COMPATIBLE_VERSION.to_string(),
        })
    );
}
