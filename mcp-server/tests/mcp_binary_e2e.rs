use base64::{engine::general_purpose::STANDARD, Engine as _};
use mcp_server::{McpBackend, McpServer};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use test_bench_support::should_skip_browser_tests;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

const REQUIRED_TOOL_NAMES: &[&str] = &[
    "navigate",
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
    fn navigate(&mut self, arguments: Value) -> anyhow::Result<Value> {
        Ok(json!({
            "status": "ok",
            "requested_url": arguments["url"],
            "final_url": arguments["url"]
        }))
    }

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

fn build_binary_once() -> anyhow::Result<std::path::PathBuf> {
    Ok(std::path::PathBuf::from(env!(
        "CARGO_BIN_EXE_dragon-head-mcp"
    )))
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
        .env(
            mcp_server::config::ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES,
            r#"["doctor-secret-phrase"]"#,
        )
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
    assert!(
        stdout.contains("prompt_injection.additional_phrases=1"),
        "stdout should report only the effective phrase count: {stdout}"
    );
    assert!(!stdout.contains("doctor-secret-phrase"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("doctor-secret-phrase"));
    for name in mcp_server::config::HONORED_CONFIG_ENV_VARS {
        assert!(stdout.contains(name), "missing {name} in: {stdout}");
    }
    Ok(())
}

#[test]
fn binary_doctor_rejects_file_additional_phrase_limits_without_disclosure() -> anyhow::Result<()> {
    let bin = build_binary_once()?;
    let dir = tempfile::tempdir()?;
    let dragon_head_dir = dir.path().join("dragon-head");
    std::fs::create_dir_all(&dragon_head_dir)?;
    let phrases = (0..=mcp_server::config::MAX_ADDITIONAL_PHRASES)
        .map(|index| format!("file-secret-phrase-{index}"))
        .map(|phrase| format!("{phrase:?}"))
        .collect::<Vec<_>>()
        .join(",");
    std::fs::write(
        dragon_head_dir.join("config.toml"),
        format!("[prompt_injection]\nadditional_phrases = [{phrases}]"),
    )?;

    let out = Command::new(&bin)
        .arg("--doctor")
        .env("XDG_CONFIG_HOME", dir.path())
        .env_remove(mcp_server::config::ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES)
        .output()?;

    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("✗ Config file"));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("file-secret-phrase"));
    assert!(!String::from_utf8_lossy(&out.stderr).contains("file-secret-phrase"));
    Ok(())
}

#[test]
fn binary_rejects_malformed_config_without_disclosing_phrase_contents() -> anyhow::Result<()> {
    let bin = build_binary_once()?;
    let dir = tempfile::tempdir()?;
    let dragon_head_dir = dir.path().join("dragon-head");
    std::fs::create_dir_all(&dragon_head_dir)?;
    std::fs::write(
        dragon_head_dir.join("config.toml"),
        "[prompt_injection]\nadditional_phrases = [\"file-parse-secret-phrase\"",
    )?;

    for args in [&["--doctor"][..], &[][..]] {
        let out = Command::new(&bin)
            .args(args)
            .env("XDG_CONFIG_HOME", dir.path())
            .env_remove(mcp_server::config::ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES)
            .output()?;

        assert!(!out.status.success());
        if args.is_empty() {
            assert!(out.stdout.is_empty(), "stdout must remain JSON-RPC clean");
        }
        assert!(!String::from_utf8_lossy(&out.stdout).contains("file-parse-secret-phrase"));
        assert!(!String::from_utf8_lossy(&out.stderr).contains("file-parse-secret-phrase"));
    }
    Ok(())
}

#[test]
fn binary_doctor_rejects_invalid_additional_phrase_env_without_disclosure() -> anyhow::Result<()> {
    let bin = build_binary_once()?;
    let dir = tempfile::tempdir()?;
    let too_many = serde_json::to_string(
        &(0..=mcp_server::config::MAX_ADDITIONAL_PHRASES)
            .map(|index| format!("doctor-secret-phrase-{index}"))
            .collect::<Vec<_>>(),
    )?;

    for raw in [
        "".to_string(),
        r#"["doctor-secret-phrase""#.to_string(),
        r#"[{"secret":"doctor-secret-phrase"}]"#.to_string(),
        too_many,
    ] {
        let out = Command::new(&bin)
            .arg("--doctor")
            .env("XDG_CONFIG_HOME", dir.path())
            .env(
                mcp_server::config::ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES,
                &raw,
            )
            .output()?;

        assert!(!out.status.success(), "raw={raw:?}");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stdout.contains("doctor-secret-phrase"), "{stdout}");
        assert!(!stderr.contains("doctor-secret-phrase"), "{stderr}");
    }
    Ok(())
}

#[test]
fn binary_startup_rejects_invalid_additional_phrase_env_without_stdout_or_disclosure(
) -> anyhow::Result<()> {
    let bin = build_binary_once()?;
    let too_many = serde_json::to_string(
        &(0..=mcp_server::config::MAX_ADDITIONAL_PHRASES)
            .map(|index| format!("startup-secret-phrase-{index}"))
            .collect::<Vec<_>>(),
    )?;
    for raw in [
        r#"["startup-secret-phrase""#.to_string(),
        r#"[{"secret":"startup-secret-phrase"}]"#.to_string(),
        too_many,
    ] {
        let out = Command::new(&bin)
            .env(
                mcp_server::config::ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES,
                &raw,
            )
            .output()?;

        assert!(!out.status.success());
        assert!(out.stdout.is_empty(), "stdout must remain JSON-RPC clean");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!stderr.contains("startup-secret-phrase"), "{stderr}");
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn binary_doctor_rejects_non_unicode_additional_phrase_env() -> anyhow::Result<()> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let bin = build_binary_once()?;
    let out = Command::new(&bin)
        .arg("--doctor")
        .env(
            mcp_server::config::ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES,
            OsString::from_vec(vec![b'[', b'"', 0xff, b'"', b']']),
        )
        .output()?;

    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("✗ Config file"));

    let out = Command::new(&bin)
        .env(
            mcp_server::config::ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES,
            OsString::from_vec(vec![b'[', b'"', 0xff, b'"', b']']),
        )
        .output()?;
    assert!(!out.status.success());
    assert!(out.stdout.is_empty(), "stdout must remain JSON-RPC clean");
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
        .env_remove(mcp_server::config::ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES)
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
        .env_remove(mcp_server::config::ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES)
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
#[ignore = "requires Chrome; starts a real browser process"]
fn test_mcp_binary_fresh_session_navigate_returns_full_delta_baseline() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let fixture = thread::spawn(move || -> anyhow::Result<()> {
        let (mut stream, _) = listener.accept()?;
        let mut request = [0_u8; 2048];
        let _ = stream.read(&mut request)?;
        let body = "<html><body><h1>Fresh navigation</h1><button>Ready</button></body></html>";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )?;
        stream.flush()?;
        Ok(())
    });

    let bin_path = std::path::PathBuf::from(env!("CARGO_BIN_EXE_dragon-head-mcp"));
    let config_home = tempfile::tempdir()?;
    let mut child = Command::new(&bin_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("XDG_CONFIG_HOME", config_home.path())
        .env("NAVIGATION_ALLOW_PRIVATE_NETWORK", "true")
        .env_remove("POLICY_FILE")
        .spawn()?;
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut stderr = child.stderr.take().expect("stderr");
    let stderr_reader = thread::spawn(move || {
        let mut output = String::new();
        let _ = stderr.read_to_string(&mut output);
        output
    });
    let mut reader = BufReader::new(stdout);
    mcp_handshake(&mut stdin, &mut reader)?;

    let requested_url = format!("http://{address}/start#ignored");
    writeln!(
        stdin,
        "{}",
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "navigate", "arguments": {"url": requested_url}}
        })
    )?;
    stdin.flush()?;
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let navigate: Value = serde_json::from_str(&line)?;
    assert_eq!(navigate["result"]["structuredContent"]["status"], "ok");
    assert_eq!(
        navigate["result"]["structuredContent"]["requested_url"],
        format!("http://{address}/start")
    );

    writeln!(
        stdin,
        "{}",
        json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {"name": "get_state", "arguments": {"delivery": "delta"}}
        })
    )?;
    stdin.flush()?;
    line.clear();
    reader.read_line(&mut line)?;
    let state: Value = serde_json::from_str(&line)?;
    assert_eq!(state["result"]["structuredContent"]["type"], "full");

    drop(stdin);
    let status = child.wait()?;
    assert!(status.success(), "dragon-head-mcp exited with {status}");
    let stderr = stderr_reader.join().expect("stderr reader panicked");
    assert!(!stderr.contains("#ignored"));
    fixture.join().expect("fixture thread panicked")?;
    Ok(())
}

#[test]
#[ignore = "requires Chrome; starts a real browser process"]
fn test_mcp_binary_get_visual_returns_png_for_clean_and_som() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let fixture = thread::spawn(move || -> anyhow::Result<()> {
        let (mut stream, _) = listener.accept()?;
        let mut request = [0_u8; 2048];
        let _ = stream.read(&mut request)?;
        let body = "<html><body><button id='ready'>Ready</button></body></html>";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )?;
        stream.flush()?;
        Ok(())
    });

    let bin_path = build_binary_once()?;
    let config_home = tempfile::tempdir()?;
    let mut child = Command::new(&bin_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("XDG_CONFIG_HOME", config_home.path())
        .env("NAVIGATION_ALLOW_PRIVATE_NETWORK", "true")
        .env_remove("POLICY_FILE")
        .spawn()?;
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut stderr = child.stderr.take().expect("stderr");
    let stderr_reader = thread::spawn(move || {
        let mut output = String::new();
        let _ = stderr.read_to_string(&mut output);
        output
    });
    let mut reader = BufReader::new(stdout);
    mcp_handshake(&mut stdin, &mut reader)?;

    writeln!(
        stdin,
        "{}",
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "navigate",
                "arguments": {"url": format!("http://{address}/visual")}
            }
        })
    )?;
    stdin.flush()?;
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let navigate: Value = serde_json::from_str(&line)?;
    assert_eq!(navigate["result"]["structuredContent"]["status"], "ok");

    for (id, mode) in [(3, "clean"), (4, "som")] {
        writeln!(
            stdin,
            "{}",
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {"name": "get_visual", "arguments": {"mode": mode}}
            })
        )?;
        stdin.flush()?;

        line.clear();
        reader.read_line(&mut line)?;
        let response: Value = serde_json::from_str(&line)?;
        let result = &response["result"];
        let content = result["content"].as_array().expect("content array");
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["mimeType"], "image/png");

        let encoded = content[1]["data"].as_str().expect("base64 PNG");
        let png = STANDARD.decode(encoded)?;
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(
            result["structuredContent"]["image_sha256"],
            hex::encode(Sha256::digest(&png))
        );
        let fallback: Value = serde_json::from_str(content[0]["text"].as_str().unwrap())?;
        assert_eq!(fallback, result["structuredContent"]);
        assert!(!content[0]["text"].as_str().unwrap().contains(encoded));

        let marks = result["structuredContent"]["marks"]
            .as_array()
            .expect("marks array");
        if mode == "clean" {
            assert!(marks.is_empty());
        } else {
            assert!(
                !marks.is_empty(),
                "SoM capture should mark the fixture button"
            );
        }
    }

    drop(stdin);
    let status = child.wait()?;
    assert!(status.success(), "dragon-head-mcp exited with {status}");
    let stderr = stderr_reader.join().expect("stderr reader panicked");
    assert!(!stderr.contains("iVBOR"), "base64 image leaked to stderr");
    fixture.join().expect("fixture thread panicked")?;
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
