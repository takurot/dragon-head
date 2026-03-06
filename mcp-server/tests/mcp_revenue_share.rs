use anyhow::Result;
use mcp_server::{AuditRetentionSnapshot, MarketplaceAttribution, McpBackend, McpServer, PlanTier};
use serde_json::{json, Value};

struct MockBackend {
    act_responses: Vec<Value>,
    act_calls: usize,
}

impl Default for MockBackend {
    fn default() -> Self {
        Self {
            act_responses: vec![json!({"status": "ok"})],
            act_calls: 0,
        }
    }
}

impl McpBackend for MockBackend {
    fn get_state(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({
            "metadata": {
                "url": "https://example.com",
                "page_instance_id": "pid",
                "state_hash": "hash",
                "load_profile": "interactive",
                "timestamp": 123
            },
            "interactive_elements": []
        }))
    }

    fn act(&mut self, _arguments: Value) -> Result<Value> {
        let response = self
            .act_responses
            .get(self.act_calls)
            .cloned()
            .or_else(|| self.act_responses.last().cloned())
            .unwrap_or_else(|| json!({"status": "ok"}));
        self.act_calls += 1;
        Ok(response)
    }

    fn verify(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"matched": true}))
    }

    fn get_visual(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"image_sha256": "abc"}))
    }

    fn ask_human(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"approved": true}))
    }

    fn run_skill(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"status": "completed"}))
    }

    fn audit_retention_snapshot(&self) -> Option<AuditRetentionSnapshot> {
        Some(AuditRetentionSnapshot {
            retained_events: 8,
            retained_bytes: 2_400_000,
        })
    }
}

#[test]
fn test_revenue_share_report_aggregates_marketplace_usage_events() -> Result<()> {
    let backend = MockBackend {
        act_responses: vec![
            json!({"status": "requires_human_approval"}),
            json!({"status": "ok"}),
        ],
        act_calls: 0,
    };
    let attribution = MarketplaceAttribution {
        pack_id: "acme.checkout.pack".to_string(),
        publisher_id: "acme-inc".to_string(),
        revenue_share_bps: 3000,
    };

    let mut server = McpServer::new_with_marketplace(backend, PlanTier::Enterprise, attribution);

    server.call_tool("get_state", json!({"format": "json"}))?;
    server.call_tool("get_state", json!({"delivery": "delta"}))?;
    server.call_tool("act", json!({"action": "click"}))?;
    server.call_tool("act", json!({"action": "click"}))?;
    server.call_tool("get_visual", json!({"mode": "som"}))?;

    let report = server.call_tool("get_revenue_share_report", json!({}))?;

    assert_eq!(report["pack_id"], json!("acme.checkout.pack"));
    assert_eq!(report["publisher_id"], json!("acme-inc"));
    assert_eq!(report["revenue_share_bps"], json!(3000));
    assert_eq!(report["event_count"], json!(6));
    assert_eq!(report["usage"]["state_generations"]["fast"], json!(1));
    assert_eq!(report["usage"]["state_generations"]["full"], json!(1));
    assert_eq!(report["usage"]["state_generations"]["delta"], json!(1));
    assert_eq!(report["usage"]["visual_captures"], json!(1));
    assert_eq!(report["usage"]["actions_executed"], json!(1));
    assert_eq!(report["usage"]["hitl_events"], json!(1));
    assert_eq!(report["gross_microusd"], json!(2430));
    assert_eq!(report["publisher_share_microusd"], json!(729));
    assert_eq!(report["platform_share_microusd"], json!(1701));

    Ok(())
}

#[test]
fn test_revenue_share_report_is_disabled_without_marketplace_context() -> Result<()> {
    let mut server = McpServer::new_with_plan(MockBackend::default(), PlanTier::Enterprise);

    let report = server.call_tool("get_revenue_share_report", json!({}))?;

    assert_eq!(report["status"], json!("marketplace_context_required"));

    Ok(())
}
