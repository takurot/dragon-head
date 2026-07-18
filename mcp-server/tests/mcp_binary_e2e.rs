use mcp_server::{McpBackend, McpServer};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{mpsc, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use test_bench_support::should_skip_browser_tests;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

const REQUIRED_TOOL_NAMES: &[&str] = &[
    "get_state",
    "act",
    "verify",
    "get_visual",
    "ask_human",
    "run_skill",
    "extract",
    "get_usage_report",
];

fn assert_required_tools(tools: &[Value]) {
    let tool_names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    for name in REQUIRED_TOOL_NAMES {
        assert!(tool_names.contains(name), "tools/list missing '{name}'");
    }
}

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

    assert_required_tools(tools);
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
    assert_eq!(content["type"], "text");
    assert!(content.get("json").is_none());
    let json_content = &resp["result"]["structuredContent"];
    let fallback: Value = serde_json::from_str(content["text"].as_str().unwrap()).unwrap();
    assert_eq!(&fallback, json_content);
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

struct ChildGuard {
    child: Child,
    stdin: Option<ChildStdin>,
}

enum StdoutEvent {
    Line(std::io::Result<String>),
    Eof,
}

impl ChildGuard {
    fn new(mut child: Child) -> Self {
        let stdin = child.stdin.take();
        Self { child, stdin }
    }

    fn stdin_mut(&mut self) -> &mut ChildStdin {
        self.stdin.as_mut().expect("failed to open stdin")
    }

    fn close_stdin(&mut self) {
        self.stdin.take();
    }

    fn wait_timeout(&mut self, timeout: Duration) -> anyhow::Result<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            if Instant::now() >= deadline {
                anyhow::bail!("dragon-head-mcp did not exit within {timeout:?}");
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.close_stdin();

        let grace_deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < grace_deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }

        #[cfg(unix)]
        {
            let process_group = -(self.child.id() as i32);
            // SAFETY: the smoke test starts the child in a fresh process group whose
            // id is the child's pid. A negative pid targets only that test-owned group.
            unsafe {
                libc::kill(process_group, libc::SIGKILL);
            }
        }
        #[cfg(not(unix))]
        {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

fn receive_stdout_line(
    receiver: &mpsc::Receiver<StdoutEvent>,
    timeout: Duration,
) -> anyhow::Result<String> {
    match receiver
        .recv_timeout(timeout)
        .map_err(|err| anyhow::anyhow!("timed out waiting for JSON-RPC stdout: {err}"))?
    {
        StdoutEvent::Line(line) => line.map_err(Into::into),
        StdoutEvent::Eof => anyhow::bail!("dragon-head-mcp closed stdout before responding"),
    }
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
// --doctor + config.toml tests (fast — no Chrome required for the config check)
// ---------------------------------------------------------------------------

#[test]
fn binary_doctor_reports_resolved_summary_for_valid_config() -> anyhow::Result<()> {
    let bin = build_binary_once()?;
    let dir = tempfile::tempdir()?;
    let dragon_head_dir = dir.path().join("dragon-head");
    std::fs::create_dir_all(&dragon_head_dir)?;
    std::fs::write(
        dragon_head_dir.join("config.toml"),
        "[prompt_injection]\nmode = \"redact\"",
    )?;

    let out = Command::new(&bin)
        .arg("--doctor")
        .env("XDG_CONFIG_HOME", dir.path())
        .env_remove("PROMPT_INJECTION_MODE")
        .output()?;

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("prompt_injection.mode=Redact"),
        "stdout should report the resolved injection mode: {stdout}"
    );
    assert!(
        !stdout.contains("✗ Config file"),
        "a valid config file must not fail the Config file check: {stdout}"
    );
    Ok(())
}

#[test]
fn binary_doctor_fails_for_invalid_injection_mode_config() -> anyhow::Result<()> {
    let bin = build_binary_once()?;
    let dir = tempfile::tempdir()?;
    let dragon_head_dir = dir.path().join("dragon-head");
    std::fs::create_dir_all(&dragon_head_dir)?;
    std::fs::write(
        dragon_head_dir.join("config.toml"),
        "[prompt_injection]\nmode = \"verbose\"",
    )?;

    let out = Command::new(&bin)
        .arg("--doctor")
        .env("XDG_CONFIG_HOME", dir.path())
        .env_remove("PROMPT_INJECTION_MODE")
        .output()?;

    assert!(
        !out.status.success(),
        "an invalid prompt_injection.mode must exit non-zero, got: {}",
        out.status
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("✗ Config file"),
        "stdout should mark the Config file check as failed: {stdout}"
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
fn test_mcp_binary_stdio_smoke() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        eprintln!("SKIP: Chrome not available");
        return Ok(());
    }

    let bin_path = build_binary_once()?;

    let t0 = Instant::now();
    let config_home = tempfile::tempdir()?;
    let mut command = Command::new(&bin_path);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .env("XDG_CONFIG_HOME", config_home.path())
        .env_remove("PROMPT_INJECTION_MODE")
        .env_remove("POLICY_FILE")
        .env_remove("AUDIT_LOG_DIR")
        .env_remove("AUDIT_LOG_MAX_BYTES")
        .env_remove("AUDIT_DURABILITY")
        .env_remove("AUDIT_LOG_STDOUT");
    #[cfg(unix)]
    command.process_group(0);

    let mut raw_child = command.spawn()?;
    let stdout = raw_child.stdout.take().expect("failed to open stdout");
    let mut child = ChildGuard::new(raw_child);
    let (stdout_sender, stdout_receiver) = mpsc::channel();
    let _stdout_reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if stdout_sender.send(StdoutEvent::Line(line)).is_err() {
                break;
            }
        }
        let _ = stdout_sender.send(StdoutEvent::Eof);
    });

    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": { "name": "mcp_binary_e2e", "version": "1.0.0" }
        }
    });
    writeln!(child.stdin_mut(), "{initialize}")?;
    child.stdin_mut().flush()?;

    let mut stdout_lines = vec![receive_stdout_line(
        &stdout_receiver,
        Duration::from_secs(120),
    )?];
    let initialize_response: Value = serde_json::from_str(&stdout_lines[0])?;
    assert_eq!(initialize_response["jsonrpc"], "2.0");
    assert_eq!(initialize_response["id"], 1);
    assert!(initialize_response.get("result").is_some());
    assert!(initialize_response.get("error").is_none());
    assert_eq!(
        initialize_response["result"]["protocolVersion"],
        "2025-11-25"
    );
    assert!(initialize_response["result"]["capabilities"]["tools"].is_object());
    assert_eq!(
        initialize_response["result"]["serverInfo"]["name"],
        "dragon-head-mcp"
    );

    let initialized = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    writeln!(child.stdin_mut(), "{initialized}")?;
    child.stdin_mut().flush()?;
    eprintln!("[timing] spawn+handshake: {:?}", t0.elapsed());

    let t1 = Instant::now();
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });
    writeln!(child.stdin_mut(), "{request}")?;
    child.stdin_mut().flush()?;

    stdout_lines.push(receive_stdout_line(
        &stdout_receiver,
        Duration::from_secs(30),
    )?);
    let response: Value = serde_json::from_str(&stdout_lines[1])?;
    eprintln!("[timing] tools/list: {:?}", t1.elapsed());

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 2);
    assert!(response.get("result").is_some());
    assert!(response.get("error").is_none());
    let tools = response["result"]["tools"]
        .as_array()
        .expect("tools is array");
    assert!(!tools.is_empty(), "tools list should not be empty");
    assert_required_tools(tools);

    child.close_stdin();
    let status = child.wait_timeout(Duration::from_secs(30))?;
    assert!(status.success(), "dragon-head-mcp exited with {status}");
    while let StdoutEvent::Line(line) = stdout_receiver
        .recv_timeout(Duration::from_secs(5))
        .map_err(|err| anyhow::anyhow!("timed out waiting for stdout EOF: {err}"))?
    {
        stdout_lines.push(line?);
    }
    assert_eq!(
        stdout_lines.len(),
        2,
        "stdout must contain exactly two JSON-RPC responses: {stdout_lines:?}"
    );
    assert!(stdout_lines.iter().all(|line| !line.trim().is_empty()));
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
    assert_eq!(content["type"], "text");
    assert!(content.get("json").is_none());
    let json_content = &response["result"]["structuredContent"];
    let fallback: Value = serde_json::from_str(content["text"].as_str().unwrap()).unwrap();
    assert_eq!(&fallback, json_content);
    assert!(json_content["plan_tier"].is_string());
    assert!(json_content["state_generations"].is_object());
    assert!(json_content["actions_executed"].is_number());

    drop(stdin);
    child.wait()?;
    eprintln!("[timing] total: {:?}", t0.elapsed());
    Ok(())
}
