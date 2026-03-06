use anyhow::Result;
use mcp_server::{AuditRetentionSnapshot, McpBackend, McpServer, PlanTier};
use serde_json::{json, Value};

struct MockBackend {
    audit_snapshot: Option<AuditRetentionSnapshot>,
    act_responses: Vec<Value>,
    act_call_count: usize,
}

impl Default for MockBackend {
    fn default() -> Self {
        Self {
            audit_snapshot: None,
            act_responses: vec![json!({"status": "ok"})],
            act_call_count: 0,
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
            .get(self.act_call_count)
            .cloned()
            .or_else(|| self.act_responses.last().cloned())
            .unwrap_or_else(|| json!({"status": "ok"}));
        self.act_call_count += 1;
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
        self.audit_snapshot
    }
}

#[test]
fn test_usage_report_tracks_metered_calls() -> Result<()> {
    let backend = MockBackend {
        audit_snapshot: Some(AuditRetentionSnapshot {
            retained_events: 12,
            retained_bytes: 4096,
        }),
        act_responses: vec![
            json!({"status": "requires_human_approval"}),
            json!({"status": "ok"}),
        ],
        act_call_count: 0,
    };
    let mut server = McpServer::new_with_plan(backend, PlanTier::Enterprise);

    server.call_tool("get_state", json!({"format": "json"}))?;
    server.call_tool("act", json!({"action": "click"}))?;
    server.call_tool("ask_human", json!({"reason": "review"}))?;
    server.call_tool("act", json!({"action": "click"}))?;
    server.call_tool("get_visual", json!({"mode": "som"}))?;

    let report = server.call_tool("get_usage_report", json!({}))?;

    assert_eq!(report["plan_tier"], json!("enterprise"));
    assert_eq!(report["state_generations"]["fast"], json!(1));
    assert_eq!(report["state_generations"]["full"], json!(1));
    assert_eq!(report["state_generations"]["delta"], json!(0));
    assert_eq!(report["visual_captures"], json!(1));
    assert_eq!(report["actions_executed"], json!(1));
    assert_eq!(report["hitl_events"], json!(1));
    assert_eq!(report["audit_retention"]["retained_events"], json!(12));
    assert_eq!(report["audit_retention"]["retained_bytes"], json!(4096));
    assert_eq!(report["cost_microusd"]["total"], json!(2690));

    Ok(())
}

#[test]
fn test_usage_report_tracks_delta_state_calls() -> Result<()> {
    let mut server = McpServer::new_with_plan(MockBackend::default(), PlanTier::Enterprise);

    server.call_tool("get_state", json!({"delivery": "delta"}))?;

    let report = server.call_tool("get_usage_report", json!({}))?;
    assert_eq!(report["state_generations"]["fast"], json!(0));
    assert_eq!(report["state_generations"]["full"], json!(0));
    assert_eq!(report["state_generations"]["delta"], json!(1));
    assert_eq!(report["cost_microusd"]["state_generations"], json!(40));

    Ok(())
}

#[test]
fn test_clean_visual_capture_does_not_increment_meter() -> Result<()> {
    let mut server = McpServer::new_with_plan(MockBackend::default(), PlanTier::Enterprise);

    server.call_tool("get_visual", json!({"mode": "clean"}))?;

    let report = server.call_tool("get_usage_report", json!({}))?;
    assert_eq!(report["visual_captures"], json!(0));
    assert_eq!(report["cost_microusd"]["visual_captures"], json!(0));

    Ok(())
}

#[test]
fn test_developer_plan_blocks_delta_delivery() -> Result<()> {
    let mut server = McpServer::new_with_plan(MockBackend::default(), PlanTier::Developer);

    let result = server.call_tool("get_state", json!({"delivery": "delta"}))?;

    assert_eq!(result["status"], json!("plan_upgrade_required"));
    assert_eq!(result["feature"], json!("semantic_delta"));
    assert_eq!(result["required_plan"], json!("pro"));
    assert_eq!(result["current_plan"], json!("developer"));

    let report = server.call_tool("get_usage_report", json!({}))?;
    assert_eq!(report["state_generations"]["delta"], json!(0));

    Ok(())
}

#[test]
fn test_developer_plan_blocks_som_visual_capture() -> Result<()> {
    let mut server = McpServer::new_with_plan(MockBackend::default(), PlanTier::Developer);

    let result = server.call_tool("get_visual", json!({"mode": "som"}))?;

    assert_eq!(result["status"], json!("plan_upgrade_required"));
    assert_eq!(result["feature"], json!("som_visual_capture"));
    assert_eq!(result["required_plan"], json!("pro"));
    assert_eq!(result["current_plan"], json!("developer"));

    let report = server.call_tool("get_usage_report", json!({}))?;
    assert_eq!(report["visual_captures"], json!(0));

    Ok(())
}

#[test]
fn test_pro_plan_blocks_enterprise_hitl_controls() -> Result<()> {
    let mut server = McpServer::new_with_plan(MockBackend::default(), PlanTier::Pro);

    let result = server.call_tool(
        "ask_human",
        json!({
            "reason": "requires human review",
            "context": true
        }),
    )?;

    assert_eq!(result["status"], json!("plan_upgrade_required"));
    assert_eq!(result["feature"], json!("policy_human_approval"));
    assert_eq!(result["required_plan"], json!("enterprise"));
    assert_eq!(result["current_plan"], json!("pro"));

    Ok(())
}
