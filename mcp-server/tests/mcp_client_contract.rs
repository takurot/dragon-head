use anyhow::Result;
use mcp_server::{McpBackend, McpServer};
use serde_json::{json, Value};

#[derive(Default)]
struct MockBackend;

impl McpBackend for MockBackend {
    fn get_state(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"ok": true, "tool": "get_state"}))
    }

    fn act(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"ok": true, "tool": "act"}))
    }

    fn verify(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"ok": true, "tool": "verify"}))
    }

    fn get_visual(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"ok": true, "tool": "get_visual"}))
    }

    fn ask_human(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"ok": true, "tool": "ask_human"}))
    }

    fn run_skill(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"ok": true, "tool": "run_skill"}))
    }
}

#[test]
fn test_mcp_contract_exposes_required_tools() {
    let server = McpServer::new(MockBackend);
    let tools = server.tools();
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();

    assert!(names.contains(&"get_state"));
    assert!(names.contains(&"act"));
    assert!(names.contains(&"verify"));
    assert!(names.contains(&"get_visual"));
    assert!(names.contains(&"ask_human"));
    assert!(names.contains(&"run_skill"));
}

#[test]
fn test_mcp_contract_all_tools_are_callable() {
    let mut server = McpServer::new(MockBackend);

    for name in [
        "get_state",
        "act",
        "verify",
        "get_visual",
        "ask_human",
        "run_skill",
    ] {
        let result = server
            .call_tool(name, json!({}))
            .unwrap_or_else(|_| panic!("tool call failed for {name}"));
        assert_eq!(result["ok"], json!(true));
        assert_eq!(result["tool"], json!(name));
    }
}
