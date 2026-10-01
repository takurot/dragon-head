//! Bounded stdio transport for the real MCP executable; protocol errors never count as success.
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const RESPONSE_LIMIT: u64 = 1024 * 1024;
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

fn validate_envelope(response: Value, id: u64) -> Result<Value> {
    ensure!(
        response["jsonrpc"] == "2.0" && response["id"] == id,
        "invalid MCP response framing"
    );
    ensure!(
        response.get("error").is_none(),
        "MCP returned a protocol error"
    );
    response
        .get("result")
        .filter(|result| result.is_object())
        .cloned()
        .context("MCP result must be an object")
}

fn validate_tool_payload(result: Value) -> Result<Value> {
    if let Some(flag) = result.get("isError") {
        ensure!(
            flag.as_bool() == Some(false),
            "MCP tool reported an error or invalid isError"
        );
    }
    let structured = result
        .get("structuredContent")
        .filter(|value| value.is_object())
        .cloned()
        .context("MCP tool structuredContent must be an object")?;
    let text = result["content"][0]["text"]
        .as_str()
        .context("MCP text fallback missing")?;
    ensure!(
        result["content"][0]["type"] == "text",
        "MCP text fallback has invalid type"
    );
    let fallback: Value = serde_json::from_str(text)
        .map_err(|_| anyhow::anyhow!("invalid MCP text fallback JSON"))?;
    ensure!(
        structured == fallback,
        "MCP structured and text results differ"
    );
    Ok(structured)
}

pub(crate) struct McpClient {
    child: Child,
    input: Option<ChildStdin>,
    responses: Option<mpsc::Receiver<Result<String>>>,
    next_id: u64,
    pub calls: Vec<String>,
    pub act_calls: Vec<Value>,
}

impl McpClient {
    pub fn start(binary: &Path, config_home: &Path) -> Result<Self> {
        let mut command = Command::new(binary);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for variable in mcp_server::config::HONORED_CONFIG_ENV_VARS {
            if *variable != "CHROME_PATH" {
                command.env_remove(variable);
            }
        }
        command.env("XDG_CONFIG_HOME", config_home);
        command.env("RUST_LOG", "error");
        let mut client = Self::spawn(command)?;
        let initialized = client.request("initialize", json!({"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"governance-bench","version":"1"}}), Duration::from_secs(120))?;
        ensure!(
            initialized["serverInfo"]["name"] == "dragon-head-mcp"
                && initialized["protocolVersion"] == "2025-11-25",
            "unexpected MCP handshake"
        );
        client.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))?;
        Ok(client)
    }

    fn spawn(mut command: Command) -> Result<Self> {
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn().context("failed to spawn MCP process")?;
        let input = child.stdin.take();
        let stdout = child.stdout.take().context("MCP stdout unavailable")?;
        let stderr = child.stderr.take().context("MCP stderr unavailable")?;
        let (sender, receiver) = mpsc::sync_channel(2);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut bytes = Vec::new();
                let result = reader
                    .by_ref()
                    .take(RESPONSE_LIMIT + 1)
                    .read_until(b'\n', &mut bytes);
                let line = match result {
                    Ok(0) => Err(anyhow::anyhow!("MCP stdout closed")),
                    Ok(count) if count as u64 > RESPONSE_LIMIT || !bytes.ends_with(b"\n") => Err(
                        anyhow::anyhow!("MCP response exceeds limit or is incomplete"),
                    ),
                    Ok(_) => String::from_utf8(bytes)
                        .map_err(|_| anyhow::anyhow!("MCP response is not UTF-8")),
                    Err(_) => Err(anyhow::anyhow!("failed to read MCP response")),
                };
                let terminal = line.is_err();
                if sender.send(line).is_err() || terminal {
                    break;
                }
            }
        });
        // Drain without retaining/logging potentially sensitive runtime diagnostics.
        thread::spawn(move || {
            let _ = std::io::copy(&mut BufReader::new(stderr), &mut std::io::sink());
        });
        Ok(Self {
            child,
            input,
            responses: Some(receiver),
            next_id: 1,
            calls: Vec::new(),
            act_calls: Vec::new(),
        })
    }

    fn send(&mut self, value: Value) -> Result<()> {
        let input = self.input.as_mut().context("MCP stdin is closed")?;
        writeln!(input, "{value}").context("failed to write MCP request")?;
        input.flush().context("failed to flush MCP request")
    }

    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
        let line = self
            .responses
            .as_ref()
            .context("MCP response channel closed")?
            .recv_timeout(timeout)
            .map_err(|_| anyhow::anyhow!("MCP response timed out or disconnected"))??;
        let response = serde_json::from_str(&line)
            .map_err(|_| anyhow::anyhow!("invalid MCP response JSON"))?;
        validate_envelope(response, id)
    }

    pub fn tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        self.calls.push(name.to_string());
        if name == "act" {
            self.act_calls.push(core_runtime::privacy::PiiRedactor::new().redact_json_tool_args(
                &json!({"target_id":arguments["target_id"],"stable_key":arguments["target_stable_key"],
                    "action":arguments["action"],"value":arguments["value"]}),
            ));
        }
        validate_tool_payload(self.request(
            "tools/call",
            json!({"name":name,"arguments":arguments}),
            CALL_TIMEOUT,
        )?)
    }

    pub fn finish(&mut self) -> Result<()> {
        self.input.take();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait()? {
                ensure!(status.success(), "MCP process exited unsuccessfully");
                return Ok(());
            }
            ensure!(Instant::now() < deadline, "MCP process shutdown timed out");
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.input.take();
        self.responses.take();
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            #[cfg(unix)]
            // SAFETY: spawn created this child's distinct process group.
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
            #[cfg(not(unix))]
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[cfg(unix)]
    fn scripted_client(script: &str) -> McpClient {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        McpClient::spawn(command).unwrap()
    }

    #[test]
    #[cfg(unix)]
    fn real_stdio_rejects_eof_timeout_malformed_and_oversized_responses() {
        for script in [
            "read line; exit 0",
            "read line; sleep 2",
            "read line; printf 'not-json\\n'",
            "read line; printf '%1048578s\\n' x",
            "read line; printf '{\"jsonrpc\":\"2.0\",\"id\":99,\"result\":{}}\\n'",
        ] {
            let mut client = scripted_client(script);
            assert!(client
                .request("initialize", json!({}), Duration::from_millis(100))
                .is_err());
        }
        let mut client = scripted_client(
            "read line; printf '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\\n'; exit 1",
        );
        assert!(client
            .request("initialize", json!({}), Duration::from_secs(2))
            .is_ok());
        assert!(client.finish().is_err());
    }

    #[test]
    fn protocol_rejects_wrong_ids_error_and_missing_results() {
        for response in [
            json!({"jsonrpc":"2.0","id":2,"result":{}}),
            json!({"jsonrpc":"2.0","id":1,"error":{"message":"private-fixture-value"}}),
            json!({"jsonrpc":"2.0","id":1}),
            json!({"jsonrpc":"1.0","id":1,"result":{}}),
            json!({"jsonrpc":"2.0","id":1,"result":{},"error":null}),
        ] {
            let error = validate_envelope(response, 1).unwrap_err();
            assert!(!error.to_string().contains("private-fixture-value"));
        }
        assert!(validate_envelope(json!({"jsonrpc":"2.0","id":1,"result":{}}), 1).is_ok());
    }

    #[test]
    fn protocol_rejects_invalid_tool_shapes_before_success() {
        for response in [
            json!({"isError":true,"structuredContent":{}}),
            json!({"isError":"false","structuredContent":{}}),
            json!({"structuredContent":null}),
            json!({"structuredContent":{},"content":[{"type":"text","text":"[]"}]}),
            json!({"structuredContent":{"status":"ok"},"content":[{"type":"text","text":"{}"}]}),
        ] {
            assert!(validate_tool_payload(response).is_err());
        }
        assert!(validate_tool_payload(json!({"structuredContent":{"status":"ok"},"content":[{"type":"text","text":"{\"status\":\"ok\"}"}]})).is_ok());
    }
}
