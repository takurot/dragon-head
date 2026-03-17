use ed25519_dalek::{Signer, SigningKey};
use marketplace::{
    DomainPack, MarketplaceMetadata, UsageEvent, calculate_revenue_share, verify_domain_pack,
};
use rand_core::OsRng;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use test_bench_support::{EvaluationBench, EvaluationMode};

#[test]
fn test_marketplace_comprehensive_evaluation_suite() -> anyhow::Result<()> {
    let mut bench = EvaluationBench::new(
        "marketplace",
        "comprehensive_evaluation",
        EvaluationMode::from_env(),
    );

    bench.run_scenario(
        "domain_pack_signature_verification",
        "signature",
        scenario_domain_pack_signature_verification,
    );
    bench.run_scenario(
        "revenue_share_accounting",
        "monetization",
        scenario_revenue_share_accounting,
    );

    bench.write_if_configured()?;
    bench.assert_required_scenarios(&[
        "domain_pack_signature_verification",
        "revenue_share_accounting",
    ])?;
    bench.assert_all_passed()?;
    Ok(())
}

fn scenario_domain_pack_signature_verification() -> anyhow::Result<Value> {
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
    assert!(verify_domain_pack(&pack, &"00".repeat(32)).is_err());

    Ok(json!({
        "pack_id": pack_id,
        "verified": true,
    }))
}

fn scenario_revenue_share_accounting() -> anyhow::Result<Value> {
    let skill_event = UsageEvent {
        pack_id: "com.example.testpack".to_string(),
        event_type: "skill_run".to_string(),
        count: 100,
    };
    let state_event = UsageEvent {
        pack_id: "com.example.testpack".to_string(),
        event_type: "state_generation".to_string(),
        count: 100,
    };

    let skill_revenue = calculate_revenue_share(&skill_event);
    let state_revenue = calculate_revenue_share(&state_event);

    assert_eq!(skill_revenue, 2.0);
    assert_eq!(state_revenue, 0.5);

    Ok(json!({
        "skill_run_revenue": skill_revenue,
        "state_generation_revenue": state_revenue,
    }))
}
