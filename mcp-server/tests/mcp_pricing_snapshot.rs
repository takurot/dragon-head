use mcp_server::{estimate_usage_cost, AuditRetentionSnapshot, PlanTier, StateGenerationUsage};
use serde_json::Value;

#[test]
fn test_usage_pricing_snapshot_diff() {
    let cost = estimate_usage_cost(
        PlanTier::Enterprise,
        &StateGenerationUsage {
            fast: 42,
            full: 17,
            delta: 128,
        },
        9,
        64,
        3,
        AuditRetentionSnapshot {
            retained_events: 700,
            retained_bytes: 3_200_000,
        },
    );

    let generated = serde_json::to_value(cost).expect("cost must serialize");
    let expected: Value =
        serde_json::from_str(include_str!("fixtures/usage_pricing_snapshot.json"))
            .expect("fixture must be valid json");

    assert_eq!(
        generated, expected,
        "usage pricing snapshot changed. If intentional, update fixtures/usage_pricing_snapshot.json"
    );
}
