use anyhow::{anyhow, Result};
use mcp_server::{McpBackend, McpServer, PlanTier};
use serde_json::{json, Value};

#[derive(Default)]
struct MockBackend;

impl McpBackend for MockBackend {
    fn get_state(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"metadata": {"url": "https://example.com"}, "interactive_elements": []}))
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
        Ok(json!({"approved": true}))
    }

    fn run_skill(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"status": "completed"}))
    }

    fn extract(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"rule": "mock", "result": null}))
    }
}

#[test]
fn test_jsonrpc_tools_list_compliance() {
    let mut server = McpServer::new(MockBackend);
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    });

    let response_raw = server
        .handle_jsonrpc(&request.to_string())
        .expect("response");
    let response: Value = serde_json::from_str(&response_raw).expect("response json");

    assert_eq!(response["jsonrpc"], json!("2.0"));
    assert_eq!(response["id"], json!(1));
    assert!(response["result"]["tools"].is_array());
}

#[test]
fn test_jsonrpc_tools_list_notification_is_noop() {
    let mut server = McpServer::new(MockBackend);
    let request = json!({
        "jsonrpc": "2.0",
        "method": "tools/list",
        "params": {}
    });
    assert!(server.handle_jsonrpc(&request.to_string()).is_none());
}

#[test]
fn test_jsonrpc_initialize_and_initialized_notification() {
    let mut server = McpServer::new(MockBackend);

    let initialize_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "1.0" }
        }
    });
    let response_raw = server
        .handle_jsonrpc(&initialize_req.to_string())
        .expect("response");
    let response: Value = serde_json::from_str(&response_raw).expect("response json");
    assert_eq!(response["jsonrpc"], json!("2.0"));
    assert_eq!(response["id"], json!(1));
    assert_eq!(response["result"]["protocolVersion"], json!("2025-11-25"));
    assert!(response["result"]["capabilities"]["tools"].is_object());
    assert_eq!(
        response["result"]["serverInfo"]["name"],
        json!("dragon-head-mcp")
    );

    let initialized_notif = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    assert!(server
        .handle_jsonrpc(&initialized_notif.to_string())
        .is_none());
}

#[test]
fn test_jsonrpc_tools_call_compliance() {
    let mut server = McpServer::new(MockBackend);
    let request = json!({
        "jsonrpc": "2.0",
        "id": "call-1",
        "method": "tools/call",
        "params": {
            "name": "verify",
            "arguments": {
                "target_id": 42,
                "expected": { "text": "Purchase" }
            }
        }
    });

    let response_raw = server
        .handle_jsonrpc(&request.to_string())
        .expect("response");
    let response: Value = serde_json::from_str(&response_raw).expect("response json");

    assert_eq!(response["jsonrpc"], json!("2.0"));
    assert_eq!(response["id"], json!("call-1"));
    assert!(response["result"]["content"].is_array());
    assert_eq!(response["result"]["content"][0]["type"], json!("json"));
    assert_eq!(
        response["result"]["content"][0]["json"]["matched"],
        json!(true)
    );
}

struct ErrorBackend {
    error_msg: &'static str,
}

impl McpBackend for ErrorBackend {
    fn get_state(&mut self, _arguments: Value) -> Result<Value> {
        Err(anyhow!("{}", self.error_msg))
    }
    fn act(&mut self, _arguments: Value) -> Result<Value> {
        Err(anyhow!("{}", self.error_msg))
    }
    fn verify(&mut self, _arguments: Value) -> Result<Value> {
        Err(anyhow!("{}", self.error_msg))
    }
    fn get_visual(&mut self, _arguments: Value) -> Result<Value> {
        Err(anyhow!("{}", self.error_msg))
    }
    fn ask_human(&mut self, _arguments: Value) -> Result<Value> {
        Err(anyhow!("{}", self.error_msg))
    }
    fn run_skill(&mut self, _arguments: Value) -> Result<Value> {
        Err(anyhow!("{}", self.error_msg))
    }

    fn extract(&mut self, _arguments: Value) -> Result<Value> {
        Err(anyhow!("{}", self.error_msg))
    }
}

#[test]
fn test_tool_call_backend_error_returns_32000() {
    let mut server = McpServer::new(ErrorBackend {
        error_msg: "internal server failure",
    });
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": "get_state", "arguments": {} }
    });
    let response_raw = server
        .handle_jsonrpc(&request.to_string())
        .expect("response");
    let response: Value = serde_json::from_str(&response_raw).expect("response json");
    assert_eq!(response["error"]["code"], json!(-32000));
}

#[test]
fn test_tool_call_params_keyword_does_not_misclassify() {
    let mut server = McpServer::new(ErrorBackend {
        error_msg: "failed to parse query params from URL",
    });
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": "get_state", "arguments": {} }
    });
    let response_raw = server
        .handle_jsonrpc(&request.to_string())
        .expect("response");
    let response: Value = serde_json::from_str(&response_raw).expect("response json");
    assert_eq!(response["error"]["code"], json!(-32000));
}

#[test]
fn test_tool_call_unknown_tool_returns_32601() {
    let mut server = McpServer::new(MockBackend);
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "name": "nonexistent_tool", "arguments": {} }
    });
    let response_raw = server
        .handle_jsonrpc(&request.to_string())
        .expect("response");
    let response: Value = serde_json::from_str(&response_raw).expect("response json");
    assert_eq!(response["error"]["code"], json!(-32601));
}

#[test]
fn test_tool_call_missing_name_returns_32602() {
    let mut server = McpServer::new(MockBackend);
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": { "arguments": {} }
    });
    let response_raw = server
        .handle_jsonrpc(&request.to_string())
        .expect("response");
    let response: Value = serde_json::from_str(&response_raw).expect("response json");
    assert_eq!(response["error"]["code"], json!(-32602));
}

#[derive(Default)]
struct CountingBackend {
    calls: usize,
}

impl McpBackend for CountingBackend {
    fn get_state(&mut self, _arguments: Value) -> Result<Value> {
        self.calls += 1;
        Ok(json!({"metadata": {}, "interactive_elements": []}))
    }
    fn act(&mut self, _arguments: Value) -> Result<Value> {
        self.calls += 1;
        Ok(json!({"status": "ok"}))
    }
    fn verify(&mut self, _arguments: Value) -> Result<Value> {
        self.calls += 1;
        Ok(json!({"matched": true}))
    }
    fn get_visual(&mut self, _arguments: Value) -> Result<Value> {
        self.calls += 1;
        Ok(json!({"mode": "som", "image_sha256": "abc"}))
    }
    fn ask_human(&mut self, _arguments: Value) -> Result<Value> {
        self.calls += 1;
        Ok(json!({"approved": true}))
    }
    fn run_skill(&mut self, _arguments: Value) -> Result<Value> {
        self.calls += 1;
        Ok(json!({"status": "completed"}))
    }
    fn extract(&mut self, _arguments: Value) -> Result<Value> {
        self.calls += 1;
        Ok(json!({"result": null}))
    }
}

fn call_tool_jsonrpc<B: McpBackend>(
    server: &mut McpServer<B>,
    name: &str,
    arguments: Value,
) -> Value {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 99,
        "method": "tools/call",
        "params": { "name": name, "arguments": arguments }
    });
    serde_json::from_str(
        &server
            .handle_jsonrpc(&request.to_string())
            .expect("tools/call response"),
    )
    .expect("response json")
}

#[test]
fn every_tool_rejects_unknown_fields_without_side_effects() {
    let cases = [
        ("get_state", json!({"unexpected": true})),
        ("act", json!({"action": "click", "unexpected": true})),
        (
            "verify",
            json!({"target_id": 1, "expected": {"text": "ok"}, "unexpected": true}),
        ),
        ("get_visual", json!({"unexpected": true})),
        (
            "ask_human",
            json!({"reason": "approve", "unexpected": true}),
        ),
        (
            "run_skill",
            json!({"skill_name": "checkout", "unexpected": true}),
        ),
        ("get_usage_report", json!({"unexpected": true})),
        ("extract", json!({"rule_name": "price", "unexpected": true})),
    ];

    for (name, arguments) in cases {
        let mut server = McpServer::new(CountingBackend::default());
        let before = server.call_tool("get_usage_report", json!({})).unwrap();
        let response = call_tool_jsonrpc(&mut server, name, arguments);
        let after = server.call_tool("get_usage_report", json!({})).unwrap();

        assert_eq!(response["error"]["code"], -32602, "tool: {name}");
        assert!(response.get("result").is_none(), "tool: {name}");
        assert_eq!(server.backend_mut().calls, 0, "tool: {name}");
        assert_eq!(before, after, "usage changed for tool: {name}");
    }
}

#[test]
fn malformed_enums_types_and_nested_fields_return_invalid_params() {
    let cases = [
        ("get_state", json!({"format": "xml"})),
        ("get_state", json!({"delivery": "detla"})),
        ("get_state", json!({"force_refresh": "true"})),
        ("get_state", json!([])),
        ("get_visual", json!({"mode": "annotated"})),
        ("get_visual", json!({"viewport": "visible"})),
        ("get_visual", json!({"mode": false})),
        (
            "act",
            json!({"action": "click", "target_id": 9_223_372_036_854_775_808_u64}),
        ),
        (
            "verify",
            json!({
                "target_id": 9_223_372_036_854_775_808_u64,
                "expected": {"text": "ok"}
            }),
        ),
        (
            "verify",
            json!({"target_id": 1, "expected": {"text": "ok", "unexpected": true}}),
        ),
        ("verify", json!({"target_id": 1, "expected": {"text": ""}})),
        ("ask_human", json!({"reason": ""})),
        ("run_skill", json!({"skill_name": "checkout", "params": []})),
        ("run_skill", json!({"skill_name": ""})),
        ("extract", json!({"rule_name": "price", "inline": {}})),
        ("extract", json!({"inline": []})),
        ("extract", json!({"inline": {}})),
        (
            "extract",
            json!({"inline": {"selector": ".item", "fields": {}}}),
        ),
        (
            "extract",
            json!({"inline": {"items": {"selector": ".item", "fields": {"price": 1}}}}),
        ),
        ("extract", json!({"rule_name": ""})),
        ("extract", json!({})),
    ];

    for (name, arguments) in cases {
        let mut server = McpServer::new(CountingBackend::default());
        let response = call_tool_jsonrpc(&mut server, name, arguments);
        assert_eq!(response["error"]["code"], -32602, "tool: {name}");
        assert_eq!(server.backend_mut().calls, 0, "tool: {name}");
    }
}

#[test]
fn invalid_visual_mode_is_rejected_before_plan_gate() {
    let mut server = McpServer::new_with_plan(CountingBackend::default(), PlanTier::Developer);
    let response = call_tool_jsonrpc(&mut server, "get_visual", json!({"mode": "invalid"}));

    assert_eq!(response["error"]["code"], -32602);
    assert_eq!(server.backend_mut().calls, 0);
}

#[test]
fn omitted_optional_fields_keep_current_defaults() {
    let mut server = McpServer::new(CountingBackend::default());

    let state = call_tool_jsonrpc(&mut server, "get_state", json!({}));
    let visual = call_tool_jsonrpc(&mut server, "get_visual", json!({}));

    assert!(state.get("result").is_some());
    assert!(visual.get("result").is_some());
    assert_eq!(server.backend_mut().calls, 2);
    let report = server.call_tool("get_usage_report", json!({})).unwrap();
    assert_eq!(report["state_generations"]["full"], 1);
    assert_eq!(report["state_generations"]["delta"], 0);
    assert_eq!(report["visual_captures"], 1);
}
