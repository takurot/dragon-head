use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};

fn chrome_available() -> bool {
    if let Ok(path) = std::env::var("CHROME_PATH") {
        if Path::new(&path).exists() {
            return true;
        }
    }
    let candidates = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
    ];
    candidates.iter().any(|p| {
        if p.contains('/') {
            Path::new(p).exists()
        } else {
            Command::new("which")
                .arg(p)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }
    })
}

fn should_skip() -> bool {
    !chrome_available()
}

fn mcp_handshake(
    stdin: &mut impl Write,
    reader: &mut BufReader<impl std::io::Read>,
) -> anyhow::Result<()> {
    // Step 1: initialize
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

    // Step 2: notifications/initialized (no response expected)
    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    writeln!(stdin, "{}", serde_json::to_string(&initialized)?)?;
    stdin.flush()?;

    Ok(())
}

#[test]
fn test_mcp_binary_full_handshake_and_tools_list() -> anyhow::Result<()> {
    if should_skip() {
        eprintln!("SKIP: Chrome not available");
        return Ok(());
    }

    let build = escargot::CargoBuild::new()
        .bin("dragon-head-mcp")
        .package("mcp-server")
        .current_release()
        .run()?;
    let bin_path = build.path();

    let mut child = Command::new(bin_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stdin = child.stdin.take().expect("failed to open stdin");
    let stdout = child.stdout.take().expect("failed to open stdout");
    let mut reader = BufReader::new(stdout);

    mcp_handshake(&mut stdin, &mut reader)?;

    // Step 3: tools/list after handshake
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
    Ok(())
}

#[test]
fn test_mcp_binary_full_handshake_and_tools_call() -> anyhow::Result<()> {
    if should_skip() {
        eprintln!("SKIP: Chrome not available");
        return Ok(());
    }

    let build = escargot::CargoBuild::new()
        .bin("dragon-head-mcp")
        .package("mcp-server")
        .current_release()
        .run()?;
    let bin_path = build.path();

    let mut child = Command::new(bin_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stdin = child.stdin.take().expect("failed to open stdin");
    let stdout = child.stdout.take().expect("failed to open stdout");
    let mut reader = BufReader::new(stdout);

    mcp_handshake(&mut stdin, &mut reader)?;

    // Step 3: tools/call after handshake
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

    assert_eq!(response["id"], 3);
    let content = &response["result"]["content"][0];
    assert_eq!(content["type"], "json");
    let json_content = &content["json"];
    assert!(json_content["plan_tier"].is_string());
    assert!(json_content["state_generations"].is_object());
    assert!(json_content["actions_executed"].is_number());

    drop(stdin);
    child.wait()?;
    Ok(())
}
