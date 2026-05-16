use anyhow::{anyhow, Result};
use mcp_server::{McpBackend, McpServer};
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
