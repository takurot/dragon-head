use mcp_server::{McpBackend, McpServer};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Instant;
use test_bench_support::should_skip_browser_tests;

// ---------------------------------------------------------------------------
// Fast library-level protocol tests (no Chrome / no subprocess)
//
// These cover initialize, notifications/initialized, tools/list, and tools/call
// without starting the binary or a browser. They exercise the same
// McpServer::handle_jsonrpc code path that the binary uses.
// ---------------------------------------------------------------------------

struct NullBackend;

impl McpBackend for NullBackend {
    fn get_state(&mut self, _: Value) -> anyhow::Result<Value> {
        Ok(json!({}))
    }
    fn act(&mut self, _: Value) -> anyhow::Result<Value> {
        Ok(json!({}))
    }
    fn verify(&mut self, _: Value) -> anyhow::Result<Value> {
        Ok(json!({}))
    }
    fn get_visual(&mut self, _: Value) -> anyhow::Result<Value> {
        Ok(json!({}))
    }
    fn ask_human(&mut self, _: Value) -> anyhow::Result<Value> {
        Ok(json!({}))
    }
    fn run_skill(&mut self, _: Value) -> anyhow::Result<Value> {
        Ok(json!({}))
    }
    fn extract(&mut self, _: Value) -> anyhow::Result<Value> {
        Ok(json!({}))
    }
}

fn null_server() -> McpServer<NullBackend> {
    McpServer::new(NullBackend)
}

/// Covers: initialize → notifications/initialized → tools/list
#[test]
fn protocol_initialize_notifications_and_tools_list() {
    let mut server = null_server();

    // initialize
    let req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "mcp_binary_e2e", "version": "1.0.0" }
        }
    });
    let resp_str = server.handle_jsonrpc(&req.to_string()).unwrap();
    let resp: Value = serde_json::from_str(&resp_str).unwrap();
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["protocolVersion"], "2025-11-25");
    assert!(resp["result"]["capabilities"]["tools"].is_object());
    assert_eq!(resp["result"]["serverInfo"]["name"], "dragon-head-mcp");

    // notifications/initialized — must return None (no response)
    let notif = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    assert!(
        server.handle_jsonrpc(&notif.to_string()).is_none(),
        "notifications/initialized must produce no response"
    );

    // tools/list
    let req = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}});
    let resp_str = server.handle_jsonrpc(&req.to_string()).unwrap();
    let resp: Value = serde_json::from_str(&resp_str).unwrap();
    assert_eq!(resp["id"], 2);
    let tools = resp["result"]["tools"].as_array().expect("tools is array");
    assert!(!tools.is_empty(), "tools list must not be empty");

    let tool_names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for name in [
        "get_state",
        "act",
        "verify",
        "get_visual",
        "ask_human",
        "run_skill",
        "get_usage_report",
    ] {
        assert!(tool_names.contains(&name), "tools/list missing '{name}'");
    }
}

/// Covers: tools/call for get_usage_report (no browser I/O needed)
#[test]
fn protocol_tools_call_get_usage_report() {
    let mut server = null_server();

    // Minimal handshake
    server.handle_jsonrpc(
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"} }
        })
        .to_string(),
    );
    server.handle_jsonrpc(
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
    );

    // tools/call → get_usage_report
    let req = json!({
        "jsonrpc": "2.0", "id": 3,
        "method": "tools/call",
        "params": { "name": "get_usage_report", "arguments": {} }
    });
    let resp_str = server.handle_jsonrpc(&req.to_string()).unwrap();
    let resp: Value = serde_json::from_str(&resp_str).unwrap();
    assert_eq!(resp["id"], 3);
    let content = &resp["result"]["content"][0];
    assert_eq!(content["type"], "json");
    let json_content = &content["json"];
    assert!(json_content["plan_tier"].is_string());
    assert!(json_content["state_generations"].is_object());
    assert!(json_content["actions_executed"].is_number());
}

// ---------------------------------------------------------------------------
// Binary helper utilities
// ---------------------------------------------------------------------------

fn mcp_handshake(
    stdin: &mut impl Write,
    reader: &mut BufReader<impl std::io::Read>,
) -> anyhow::Result<()> {
    let initialize = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "mcp_binary_e2e", "version": "1.0.0" }
        }
    });
    writeln!(stdin, "{}", serde_json::to_string(&initialize)?)?;
    stdin.flush()?;

    let mut line = String::new();
    reader.read_line(&mut line)?;
    let resp: serde_json::Value = serde_json::from_str(&line)?;
    assert_eq!(resp["id"], 1);
    assert_eq!(resp["result"]["protocolVersion"], "2025-11-25");
    assert!(resp["result"]["capabilities"]["tools"].is_object());
    assert_eq!(resp["result"]["serverInfo"]["name"], "dragon-head-mcp");

    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    writeln!(stdin, "{}", serde_json::to_string(&initialized)?)?;
    stdin.flush()?;

    Ok(())
}

/// Build the binary once per test-binary invocation to avoid redundant recompiles.
fn build_binary_once() -> anyhow::Result<std::path::PathBuf> {
    static BIN: OnceLock<std::path::PathBuf> = OnceLock::new();
    if let Some(path) = BIN.get() {
        return Ok(path.clone());
    }
    let build = escargot::CargoBuild::new()
        .bin("dragon-head-mcp")
        .package("mcp-server")
        .current_release()
        .run()?;
    let path = build.path().to_owned();
    let _ = BIN.set(path.clone());
    Ok(path)
}

// ---------------------------------------------------------------------------
// --init flag tests (fast — no Chrome)
// ---------------------------------------------------------------------------

#[test]
fn binary_init_no_arg_outputs_all_clients() -> anyhow::Result<()> {
    let bin = build_binary_once()?;
    let out = Command::new(&bin).arg("--init").output()?;
    assert!(out.status.success(), "exit status: {}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    for client in ["claude-desktop", "claude-code", "codex", "generic"] {
        assert!(
            stdout.contains(client),
            "output missing section for '{client}'"
        );
    }
    assert!(
        stdout.contains("dragon-head-mcp"),
        "output must reference the binary name"
    );
    Ok(())
}

#[test]
fn binary_init_claude_desktop_outputs_json() -> anyhow::Result<()> {
    let bin = build_binary_once()?;
    let out = Command::new(&bin)
        .args(["--init", "claude-desktop"])
        .output()?;
    assert!(out.status.success(), "exit status: {}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("dragon-head-mcp"),
        "stdout must contain the binary name"
    );
    assert!(
        stdout.contains("mcpServers"),
        "stdout must contain mcpServers key"
    );
    Ok(())
}

#[test]
fn binary_init_unknown_client_exits_nonzero() -> anyhow::Result<()> {
    let bin = build_binary_once()?;
    let out = Command::new(&bin)
        .args(["--init", "not-a-real-client"])
        .output()?;
    assert!(
        !out.status.success(),
        "unknown client should exit non-zero, got: {}",
        out.status
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unknown client"),
        "stderr should mention 'unknown client', got: {stderr}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Heavy binary E2E tests — spawn the real binary with Chrome
//
// These are marked #[ignore] because Chrome startup takes 30-60s per test,
// which exceeds Rust's 60s long-test warning threshold and makes normal
// `cargo test --workspace` feedback slow.
//
// Run explicitly when needed:
//   cargo test -p mcp-server --test mcp_binary_e2e -- --ignored
//   cargo test -p mcp-server --test mcp_binary_e2e test_mcp_binary -- --ignored
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires Chrome; starts a real browser process (~30-60s). Run with: cargo test -p mcp-server --test mcp_binary_e2e -- --ignored"]
fn test_mcp_binary_full_handshake_and_tools_list() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        eprintln!("SKIP: Chrome not available");
        return Ok(());
    }

    let bin_path = build_binary_once()?;

    let t0 = Instant::now();
    let mut child = Command::new(&bin_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stdin = child.stdin.take().expect("failed to open stdin");
    let stdout = child.stdout.take().expect("failed to open stdout");
    let mut reader = BufReader::new(stdout);

    mcp_handshake(&mut stdin, &mut reader)?;
    eprintln!("[timing] spawn+handshake: {:?}", t0.elapsed());

    let t1 = Instant::now();
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });
    writeln!(stdin, "{}", serde_json::to_string(&request)?)?;
    stdin.flush()?;

    let mut response_line = String::new();
    reader.read_line(&mut response_line)?;
    let response: serde_json::Value = serde_json::from_str(&response_line)?;
    eprintln!("[timing] tools/list: {:?}", t1.elapsed());

    assert_eq!(response["id"], 2);
    assert!(response["result"]["tools"].is_array());
    let tools = response["result"]["tools"]
        .as_array()
        .expect("tools is array");
    assert!(!tools.is_empty(), "tools list should not be empty");

    let tool_names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(tool_names.contains(&"get_state"));
    assert!(tool_names.contains(&"act"));
    assert!(tool_names.contains(&"verify"));
    assert!(tool_names.contains(&"get_visual"));
    assert!(tool_names.contains(&"ask_human"));
    assert!(tool_names.contains(&"run_skill"));
    assert!(tool_names.contains(&"get_usage_report"));

    drop(stdin);
    child.wait()?;
    eprintln!("[timing] total: {:?}", t0.elapsed());
    Ok(())
}

#[test]
#[ignore = "requires Chrome; starts a real browser process (~30-60s). Run with: cargo test -p mcp-server --test mcp_binary_e2e -- --ignored"]
fn test_mcp_binary_full_handshake_and_tools_call() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        eprintln!("SKIP: Chrome not available");
        return Ok(());
    }

    let bin_path = build_binary_once()?;

    let t0 = Instant::now();
    let mut child = Command::new(&bin_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stdin = child.stdin.take().expect("failed to open stdin");
    let stdout = child.stdout.take().expect("failed to open stdout");
    let mut reader = BufReader::new(stdout);

    mcp_handshake(&mut stdin, &mut reader)?;
    eprintln!("[timing] spawn+handshake: {:?}", t0.elapsed());

    let t1 = Instant::now();
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "get_usage_report",
            "arguments": {}
        }
    });
    writeln!(stdin, "{}", serde_json::to_string(&request)?)?;
    stdin.flush()?;

    let mut response_line = String::new();
    reader.read_line(&mut response_line)?;
    let response: serde_json::Value = serde_json::from_str(&response_line)?;
    eprintln!("[timing] tools/call: {:?}", t1.elapsed());

    assert_eq!(response["id"], 3);
    let content = &response["result"]["content"][0];
    assert_eq!(content["type"], "json");
    let json_content = &content["json"];
    assert!(json_content["plan_tier"].is_string());
    assert!(json_content["state_generations"].is_object());
    assert!(json_content["actions_executed"].is_number());

    drop(stdin);
    child.wait()?;
    eprintln!("[timing] total: {:?}", t0.elapsed());
    Ok(())
}
