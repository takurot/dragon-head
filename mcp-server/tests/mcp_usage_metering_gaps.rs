/// Tests for ISSUE-06: McpServer usage metering gaps.
///
/// Verifies that:
/// 1. `ask_human` tool calls are counted as HITL events when approved.
/// 2. `run_skill` execution propagates act and visual-capture counts back to McpServer.
use anyhow::Result;
use mcp_server::{McpBackend, McpServer, PlanTier, SkillUsageDelta};
use serde_json::{json, Value};

// --- Shared mock backend ---

struct MockBackend {
    ask_human_response: Value,
    skill_delta: SkillUsageDelta,
}

impl Default for MockBackend {
    fn default() -> Self {
        Self {
            ask_human_response: json!({"approved": true, "pending": false}),
            skill_delta: SkillUsageDelta::default(),
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
                "timestamp": 0
            },
            "interactive_elements": []
        }))
    }

    fn act(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"status": "ok"}))
    }

    fn verify(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"matched": true}))
    }

    fn get_visual(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"image_sha256": "abc"}))
    }

    fn ask_human(&mut self, _arguments: Value) -> Result<Value> {
        Ok(self.ask_human_response.clone())
    }

    fn run_skill(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"status": "completed", "message": null, "trace": []}))
    }

    fn extract(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"rule": "mock", "result": null}))
    }

    fn take_skill_usage_delta(&mut self) -> SkillUsageDelta {
        std::mem::take(&mut self.skill_delta)
    }
}

// --- ask_human metering ---

#[test]
fn ask_human_approved_records_hitl_event() -> Result<()> {
    let backend = MockBackend {
        ask_human_response: json!({"approved": true, "pending": false}),
        ..Default::default()
    };
    let mut server = McpServer::new_with_plan(backend, PlanTier::Enterprise);

    server.call_tool("ask_human", json!({"reason": "needs approval"}))?;

    let report = server.call_tool("get_usage_report", json!({}))?;
    assert_eq!(
        report["hitl_events"],
        json!(1),
        "approved ask_human should record one hitl_event"
    );
    Ok(())
}

#[test]
fn ask_human_not_approved_does_not_record_hitl_event() -> Result<()> {
    let backend = MockBackend {
        ask_human_response: json!({"approved": false, "pending": false}),
        ..Default::default()
    };
    let mut server = McpServer::new_with_plan(backend, PlanTier::Enterprise);

    server.call_tool("ask_human", json!({"reason": "nothing pending"}))?;

    let report = server.call_tool("get_usage_report", json!({}))?;
    assert_eq!(
        report["hitl_events"],
        json!(0),
        "unapproved ask_human should not record a hitl_event"
    );
    Ok(())
}

// --- run_skill metering ---

#[test]
fn run_skill_records_actions_from_skill_steps() -> Result<()> {
    let backend = MockBackend {
        skill_delta: SkillUsageDelta {
            actions_executed: 3,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut server = McpServer::new_with_plan(backend, PlanTier::Enterprise);

    server.call_tool("run_skill", json!({"skill_name": "my_skill"}))?;

    let report = server.call_tool("get_usage_report", json!({}))?;
    assert_eq!(
        report["actions_executed"],
        json!(3),
        "skill act steps should be counted in actions_executed"
    );
    Ok(())
}

#[test]
fn run_skill_records_visual_captures_from_skill_steps() -> Result<()> {
    let backend = MockBackend {
        skill_delta: SkillUsageDelta {
            visual_captures: 2,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut server = McpServer::new_with_plan(backend, PlanTier::Enterprise);

    server.call_tool("run_skill", json!({"skill_name": "my_skill"}))?;

    let report = server.call_tool("get_usage_report", json!({}))?;
    assert_eq!(
        report["visual_captures"],
        json!(2),
        "skill visual steps should be counted in visual_captures"
    );
    Ok(())
}

#[test]
fn run_skill_records_hitl_events_from_skill_steps() -> Result<()> {
    let backend = MockBackend {
        skill_delta: SkillUsageDelta {
            hitl_events: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut server = McpServer::new_with_plan(backend, PlanTier::Enterprise);

    server.call_tool("run_skill", json!({"skill_name": "my_skill"}))?;

    let report = server.call_tool("get_usage_report", json!({}))?;
    assert_eq!(
        report["hitl_events"],
        json!(1),
        "skill hitl steps should be counted in hitl_events"
    );
    Ok(())
}

#[test]
fn run_skill_delta_accumulates_with_direct_tool_calls() -> Result<()> {
    let backend = MockBackend {
        skill_delta: SkillUsageDelta {
            actions_executed: 2,
            visual_captures: 1,
            hitl_events: 0,
        },
        ..Default::default()
    };
    let mut server = McpServer::new_with_plan(backend, PlanTier::Enterprise);

    // Direct act call
    server.call_tool("act", json!({"action": "click"}))?;
    // Skill run with internal acts
    server.call_tool("run_skill", json!({"skill_name": "my_skill"}))?;

    let report = server.call_tool("get_usage_report", json!({}))?;
    assert_eq!(
        report["actions_executed"],
        json!(3),
        "direct act + skill acts should accumulate: 1 + 2 = 3"
    );
    assert_eq!(
        report["visual_captures"],
        json!(1),
        "skill visual captures should appear in total"
    );
    Ok(())
}

#[test]
fn run_skill_delta_is_consumed_after_call() -> Result<()> {
    // Two run_skill calls with independent deltas
    // The second call has a fresh delta (0); if take_skill_usage_delta is
    // called correctly each time, total should be 2, not 4.
    let backend = MockBackend {
        skill_delta: SkillUsageDelta {
            actions_executed: 2,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut server = McpServer::new_with_plan(backend, PlanTier::Enterprise);

    server.call_tool("run_skill", json!({"skill_name": "skill_a"}))?;
    // Second call: delta was consumed, backend returns zero delta
    server.call_tool("run_skill", json!({"skill_name": "skill_b"}))?;

    let report = server.call_tool("get_usage_report", json!({}))?;
    assert_eq!(
        report["actions_executed"],
        json!(2),
        "delta should be consumed after first run_skill; second call adds 0"
    );
    Ok(())
}

// --- plan-gate edge cases ---

#[test]
fn ask_human_plan_blocked_does_not_record_hitl_event() -> Result<()> {
    // Developer tier does not have PolicyHumanApproval; ask_human returns a plan-gate
    // response and record_usage is never called, so hitl_events stays at 0.
    let mut server = McpServer::new_with_plan(MockBackend::default(), PlanTier::Developer);

    let result = server.call_tool("ask_human", json!({"reason": "needs approval"}))?;

    assert_eq!(
        result["status"],
        json!("plan_upgrade_required"),
        "Developer tier should be blocked from ask_human"
    );

    let report = server.call_tool("get_usage_report", json!({}))?;
    assert_eq!(
        report["hitl_events"],
        json!(0),
        "plan-blocked ask_human must not record a hitl_event"
    );
    Ok(())
}
