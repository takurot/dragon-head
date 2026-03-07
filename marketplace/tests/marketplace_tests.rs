use ed25519_dalek::{Signer, SigningKey};
use marketplace::{
    DomainPack, MarketplaceMetadata, UsageEvent, calculate_revenue_share, verify_domain_pack,
};
use rand_core::OsRng;
use sha2::{Digest, Sha256};

#[test]
fn test_signature_verification() {
    let mut csprng = OsRng;
    let signing_key = SigningKey::generate(&mut csprng);
    let pubkey_bytes = signing_key.verifying_key().to_bytes();
    let pubkey_hex = hex::encode(pubkey_bytes);

    let pack_id = "com.example.testpack";
    let author = "Test Author";
    let version = "1.0.0";

    let mut hasher = Sha256::new();
    hasher.update(pack_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(author.as_bytes());
    hasher.update(b"\0");
    hasher.update(version.as_bytes());
    let payload = hasher.finalize();

    let signature = signing_key.sign(&payload);
    let signature_hex = hex::encode(signature.to_bytes());

    let metadata = MarketplaceMetadata {
        pack_id: pack_id.to_string(),
        author: author.to_string(),
        version: version.to_string(),
        compatible_version: "2.1".to_string(),
        dependencies: vec![],
        signature: Some(signature_hex),
    };

    let pack = DomainPack {
        metadata,
        plugin: None,
        skills: vec![],
    };

    assert!(verify_domain_pack(&pack, &pubkey_hex).is_ok());

    // Test with invalid public key
    let invalid_pubkey = "00".repeat(32);
    assert!(verify_domain_pack(&pack, &invalid_pubkey).is_err());
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
}
