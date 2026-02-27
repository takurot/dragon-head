use anyhow::Result;
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

    let response_raw = server.handle_jsonrpc(&request.to_string());
    let response: Value = serde_json::from_str(&response_raw).expect("response json");

    assert_eq!(response["jsonrpc"], json!("2.0"));
    assert_eq!(response["id"], json!(1));
    assert!(response["result"]["tools"].is_array());
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

    let response_raw = server.handle_jsonrpc(&request.to_string());
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
