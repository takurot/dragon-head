use anyhow::{Context, Result};
use core_runtime::{ApprovalScope, BrowserClient, PolicyAction, PolicyRule};
use mcp_server::{AuditRetentionSnapshot, CoreRuntimeBackend, McpBackend, McpServer, PlanTier};
use serde_json::{json, Value};
use test_bench_support::{EvaluationBench, EvaluationMode};
use core_runtime::should_skip_browser_tests;

#[test]
fn test_mcp_server_comprehensive_evaluation_suite() -> Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let mut bench = EvaluationBench::new(
        "mcp-server",
        "comprehensive_evaluation",
        EvaluationMode::from_env(),
    );

    bench.run_scenario(
        "tool_flow_state_and_act",
        "tool_flow",
        scenario_tool_flow_state_and_act,
    );
    bench.run_scenario("hitl_flow", "hitl", scenario_hitl_flow);
    bench.run_scenario(
        "usage_report_plan_gating",
        "metering",
        scenario_usage_report_plan_gating,
    );

    bench.write_if_configured()?;
    bench.assert_required_scenarios(&[
        "tool_flow_state_and_act",
        "hitl_flow",
        "usage_report_plan_gating",
    ])?;
    bench.assert_all_passed()?;

    Ok(())
}

fn scenario_tool_flow_state_and_act() -> Result<Value> {
    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <button id="purchase" onclick="document.body.dataset.clicked='yes'">Purchase</button>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let mut server = McpServer::new(CoreRuntimeBackend::new(page));
    let state = server.call_tool(
        "get_state",
        json!({
            "format": "json",
            "force_refresh": true
        }),
    )?;

    let target_id = state["interactive_elements"][0]["id"]
        .as_i64()
        .context("target id should exist")?;
    let stable_key = state["interactive_elements"][0]["stable_key"]
        .as_str()
        .context("stable key should exist")?
        .to_string();

    let act = server.call_tool(
        "act",
        json!({
            "target_id": target_id,
            "target_stable_key": stable_key,
            "action": "click"
        }),
    )?;
    assert_eq!(act["status"], json!("ok"));

    let clicked = server
        .backend_mut()
        .page()
        .evaluate_script("document.body.dataset.clicked")?
        .value
        .and_then(|value| value.as_str().map(ToOwned::to_owned));
    assert_eq!(clicked.as_deref(), Some("yes"));

    Ok(json!({
        "interactive_elements": state["interactive_elements"].as_array().map_or(0, Vec::len),
        "clicked": true,
    }))
}

fn scenario_hitl_flow() -> Result<Value> {
    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    page.set_policy_rules(vec![PolicyRule {
        id: "checkout-approval".to_string(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_string()),
        text_regex: Some("(?i)purchase".to_string()),
        context_regex: None,
        action: PolicyAction::RequireHumanApproval,
        scope: Some(ApprovalScope::ActionOnly),
    }])?;

    let html = r#"
        <html>
            <body>
                <button id="purchase" onclick="document.body.dataset.clicked='yes'">Purchase</button>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let mut server = McpServer::new(CoreRuntimeBackend::new(page));
    let state = server.call_tool(
        "get_state",
        json!({
            "format": "json",
            "force_refresh": true
        }),
    )?;

    let target_id = state["interactive_elements"][0]["id"]
        .as_i64()
        .context("target id should exist in hitl state")?;
    let stable_key = state["interactive_elements"][0]["stable_key"]
        .as_str()
        .context("stable key should exist in hitl state")?
        .to_string();

    let first_act = server.call_tool(
        "act",
        json!({
            "target_id": target_id,
            "target_stable_key": stable_key,
            "action": "click"
        }),
    )?;
    assert_eq!(first_act["status"], json!("requires_human_approval"));

    let approval = server.call_tool(
        "ask_human",
        json!({
            "reason": "checkout requires approval",
            "context": true
        }),
    )?;
    assert_eq!(approval["approved"], json!(true));

    let second_act = server.call_tool(
        "act",
        json!({
            "target_id": target_id,
            "target_stable_key": stable_key,
            "action": "click"
        }),
    )?;
    assert_eq!(second_act["status"], json!("ok"));

    Ok(json!({
        "approved": true,
        "rule_id": approval["rule_id"],
    }))
}

fn scenario_usage_report_plan_gating() -> Result<Value> {
    let mut server = McpServer::new_with_plan(
        MockBackend {
            audit_snapshot: Some(AuditRetentionSnapshot {
                retained_events: 12,
                retained_bytes: 4096,
            }),
            act_responses: vec![
                json!({"status": "requires_human_approval"}),
                json!({"status": "ok"}),
            ],
            act_call_count: 0,
        },
        PlanTier::Enterprise,
    );

    server.call_tool("get_state", json!({"format": "json"}))?;
    server.call_tool("act", json!({"action": "click"}))?;
    server.call_tool("ask_human", json!({"reason": "review"}))?;
    server.call_tool("act", json!({"action": "click"}))?;
    server.call_tool("get_visual", json!({"mode": "som"}))?;

    let enterprise_report = server.call_tool("get_usage_report", json!({}))?;
    assert_eq!(enterprise_report["actions_executed"], json!(1));
    assert_eq!(enterprise_report["hitl_events"], json!(1));

    let mut developer = McpServer::new_with_plan(MockBackend::default(), PlanTier::Developer);
    let visual_gate = developer.call_tool("get_visual", json!({"mode": "som"}))?;
    assert_eq!(visual_gate["status"], json!("plan_upgrade_required"));
    assert_eq!(visual_gate["feature"], json!("som_visual_capture"));

    Ok(json!({
        "enterprise_total_cost": enterprise_report["cost_microusd"]["total"],
        "developer_gate": visual_gate["feature"],
    }))
}

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
