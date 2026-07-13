use ed25519_dalek::{Signer, SigningKey};
use marketplace::{
    DomainPack, MarketplaceMetadata, UsageEvent, calculate_revenue_share,
    domain_pack_signature_payload, encode_domain_pack_signature, verify_domain_pack,
};
use rand_core::OsRng;
use serde_json::{Value, json};
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

    let metadata = MarketplaceMetadata {
        pack_id: "com.example.testpack".to_string(),
        author: "Test Author".to_string(),
        version: "1.0.0".to_string(),
        compatible_version: "2.1".to_string(),
        dependencies: vec![],
        signature: None,
    };

    let mut pack = DomainPack {
        metadata,
        plugin: None,
        skills: vec![],
    };
    let payload = domain_pack_signature_payload(&pack)?;
    pack.metadata.signature = Some(encode_domain_pack_signature(&signing_key.sign(&payload)));

    assert!(verify_domain_pack(&pack, &pubkey_hex).is_ok());
    assert!(verify_domain_pack(&pack, &"00".repeat(32)).is_err());
    pack.metadata.compatible_version = "tampered".to_string();
    assert!(verify_domain_pack(&pack, &pubkey_hex).is_err());

    Ok(json!({
        "pack_id": pack.metadata.pack_id,
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
