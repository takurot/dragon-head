use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};

use core_runtime::{ApprovalScope, BrowserClient, PolicyAction, PolicyRule};
use mcp_server::{AuditRetentionSnapshot, CoreRuntimeBackend, McpBackend, McpServer, PlanTier};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use test_bench_support::{EvaluationBench, EvaluationMode};

#[test]
fn test_mcp_server_comprehensive_evaluation_suite() -> Result<()> {
    if test_bench_support::should_skip_browser_tests() {
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
    bench.run_scenario(
        "delta_delivery_full_seeds_baseline",
        "delta",
        scenario_delta_delivery_full_seeds_baseline,
    );
    bench.run_scenario(
        "navigate_contract_and_metering",
        "navigation",
        scenario_navigate_contract_and_metering,
    );
    bench.run_scenario(
        "visual_image_content_contract",
        "visual",
        scenario_visual_image_content_contract,
    );

    bench.write_if_configured()?;
    bench.assert_required_scenarios(&[
        "tool_flow_state_and_act",
        "hitl_flow",
        "usage_report_plan_gating",
        "delta_delivery_full_seeds_baseline",
        "navigate_contract_and_metering",
        "visual_image_content_contract",
    ])?;
    bench.assert_all_passed()?;

    Ok(())
}

fn scenario_visual_image_content_contract() -> Result<Value> {
    let png = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO5dM2QAAAAASUVORK5CYII=")?;
    let mut server = McpServer::new(MockBackend {
        visual_image: Some(png.clone()),
        ..Default::default()
    });
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": "get_visual", "arguments": {"mode": "som"}}
    });
    let response: Value = serde_json::from_str(
        &server
            .handle_jsonrpc(&request.to_string())
            .context("get_visual response")?,
    )?;
    let content = response["result"]["content"]
        .as_array()
        .context("content array")?;
    let decoded = STANDARD.decode(content[1]["data"].as_str().context("image data")?)?;
    assert_eq!(decoded, png);
    assert_eq!(content[1]["mimeType"], "image/png");
    assert_eq!(
        response["result"]["structuredContent"]["image_sha256"],
        hex::encode(Sha256::digest(&decoded))
    );

    Ok(json!({
        "content_blocks": content.len(),
        "mime_type": content[1]["mimeType"]
    }))
}

fn scenario_navigate_contract_and_metering() -> Result<Value> {
    let mut server = McpServer::new(MockBackend::default());
    let response = server.call_tool("navigate", json!({"url": "https://example.com/start"}))?;
    assert_eq!(response["status"], "ok");
    assert_eq!(response["requested_url"], "https://example.com/start");
    let usage = server.call_tool("get_usage_report", json!({}))?;
    assert_eq!(usage["actions_executed"], 1);

    Ok(json!({
        "status": response["status"],
        "actions_executed": usage["actions_executed"]
    }))
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
        outcome_projector: None,
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

/// Verifies that CoreRuntimeBackend seeds previous_semantic_state on a Full delivery,
/// so a subsequent Delta call on the same unchanged page returns `type: no_change`.
fn scenario_delta_delivery_full_seeds_baseline() -> Result<Value> {
    use mcp_server::PlanTier;

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"<html><body><button>Click me</button></body></html>"#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let mut server = McpServer::new_with_plan(CoreRuntimeBackend::new(page), PlanTier::Pro);

    // Full delivery seeds previous_semantic_state.
    let full = server.call_tool("get_state", json!({"delivery": "full"}))?;
    assert!(
        full["metadata"].is_object(),
        "full delivery must return metadata"
    );

    // Delta call on unchanged page must return no_change, not a spurious full.
    let delta = server.call_tool("get_state", json!({"delivery": "delta"}))?;
    assert_eq!(
        delta["type"],
        json!("no_change"),
        "delta after full delivery of same page must return no_change"
    );

    Ok(json!({ "delta_type": delta["type"] }))
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
            visual_image: None,
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
    // 2 hitl_events: act returning requires_human_approval + ask_human returning approved=true
    assert_eq!(enterprise_report["hitl_events"], json!(2));

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
    visual_image: Option<Vec<u8>>,
}

impl Default for MockBackend {
    fn default() -> Self {
        Self {
            audit_snapshot: None,
            act_responses: vec![json!({"status": "ok"})],
            act_call_count: 0,
            visual_image: None,
        }
    }
}

impl McpBackend for MockBackend {
    fn navigate(&mut self, arguments: Value) -> Result<Value> {
        Ok(
            json!({"status": "ok", "requested_url": arguments["url"], "final_url": arguments["url"]}),
        )
    }

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
        let image_sha256 = self
            .visual_image
            .as_deref()
            .map(|image| hex::encode(Sha256::digest(image)))
            .unwrap_or_else(|| "abc".to_string());
        Ok(json!({"image_sha256": image_sha256}))
    }

    fn ask_human(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"approved": true}))
    }

    fn run_skill(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"status": "completed"}))
    }

    fn extract(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"rule": "mock", "result": null}))
    }

    fn take_visual_image(&mut self) -> Option<Vec<u8>> {
        self.visual_image.take()
    }

    fn audit_retention_snapshot(&self) -> Option<AuditRetentionSnapshot> {
        self.audit_snapshot
    }
}
