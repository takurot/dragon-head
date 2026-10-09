//! Runs the shipped MCP process, Chrome, embedded HITL bridge, and signed webhook.
//! Only the Slack API service and human reviewer are local scripted doubles.

use anyhow::{ensure, Context, Result};
use axum::{extract::State, routing::get, routing::post, Json, Router};
use core_runtime::{OutcomeProjectorConfig, PolicyAction, PolicyRule};
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::{
    io::{BufRead, BufReader, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const EMAIL: &str = "demo-payer@example.com";
const PASSWORD: &str = "demo-password-private";
const CARD: &str = "4111111111111111";
const SECRET: &str = "demo-local-signing-secret";
const CHANNEL: &str = "demo-channel";
const MESSAGE_TS: &str = "1234567890.000001";
const WAIT: Duration = Duration::from_secs(30);

#[derive(Clone)]
struct LocalState {
    requests: mpsc::Sender<(&'static str, Value)>,
    submissions: Arc<AtomicUsize>,
    submitted_bodies: Arc<Mutex<Vec<Value>>>,
}

struct LocalServices {
    address: SocketAddr,
    requests: mpsc::Receiver<(&'static str, Value)>,
    submissions: Arc<AtomicUsize>,
    submitted_bodies: Arc<Mutex<Vec<Value>>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    worker: Option<thread::JoinHandle<Result<()>>>,
}

impl LocalServices {
    fn start() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let (requests_tx, requests) = mpsc::channel();
        let submissions = Arc::new(AtomicUsize::new(0));
        let submitted_bodies = Arc::new(Mutex::new(Vec::new()));
        let state = LocalState {
            requests: requests_tx,
            submissions: Arc::clone(&submissions),
            submitted_bodies: Arc::clone(&submitted_bodies),
        };
        let (shutdown, stopped) = tokio::sync::oneshot::channel();
        let worker = thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async move {
                let app = Router::new()
                    .route(
                        "/payment",
                        get(|| async {
                            axum::response::Html(include_str!("fixtures/high-risk-payment.html"))
                        }),
                    )
                    .route("/api/chat.postMessage", post(post_message))
                    .route("/api/chat.update", post(update_message))
                    .route("/submitted", post(submitted))
                    .with_state(state);
                let listener = tokio::net::TcpListener::from_std(listener)?;
                // Cancel fixture tasks on cleanup instead of waiting forever for keep-alives.
                tokio::select! {
                    result = async { axum::serve(listener, app).await } => result?,
                    _ = stopped => {},
                }
                Ok(())
            })
        });
        Ok(Self {
            address,
            requests,
            submissions,
            submitted_bodies,
            shutdown: Some(shutdown),
            worker: Some(worker),
        })
    }

    fn next_request(&self, expected_method: &str) -> Result<Value> {
        let (method, body) = self
            .requests
            .recv_timeout(WAIT)
            .with_context(|| format!("waiting for Slack API {expected_method}"))?;
        ensure!(
            method == expected_method,
            "unexpected Slack API method {method}"
        );
        ensure!(body["channel"] == CHANNEL, "unexpected Slack channel");
        Ok(body)
    }
}

impl Drop for LocalServices {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

async fn post_message(State(state): State<LocalState>, Json(body): Json<Value>) -> Json<Value> {
    let _ = state.requests.send(("chat.postMessage", body));
    Json(json!({"ok": true, "ts": MESSAGE_TS}))
}

async fn update_message(State(state): State<LocalState>, Json(body): Json<Value>) -> Json<Value> {
    let _ = state.requests.send(("chat.update", body));
    Json(json!({"ok": true}))
}

async fn submitted(State(state): State<LocalState>, Json(body): Json<Value>) -> Json<Value> {
    state.submitted_bodies.lock().unwrap().push(body);
    state.submissions.fetch_add(1, Ordering::SeqCst);
    Json(json!({"ok": true}))
}

struct McpProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    responses: mpsc::Receiver<std::io::Result<String>>,
    next_id: u64,
}

impl McpProcess {
    fn start(config_home: &Path) -> Result<Self> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_dragon-head-mcp"));
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for variable in mcp_server::config::HONORED_CONFIG_ENV_VARS {
            // Retain explicit Chrome discovery from the caller; isolate all other settings.
            if *variable != "CHROME_PATH" {
                command.env_remove(variable);
            }
        }
        command
            .env("XDG_CONFIG_HOME", config_home)
            .env("SLACK_SIGNING_SECRET", SECRET)
            .env("SLACK_BOT_TOKEN", "demo-dummy-token")
            .env("SLACK_CHANNEL", CHANNEL)
            .env_remove("AUDIT_LOG_STDOUT");
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command.spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().context("child stdout")?;
        let (sender, responses) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut process = Self {
            child,
            stdin,
            responses,
            next_id: 1,
        };
        let initialized = process.request(
            "initialize",
            json!({
                "protocolVersion": "2025-11-25", "capabilities": {},
                "clientInfo": {"name": "high-risk-demo", "version": "1"}
            }),
            Duration::from_secs(120),
        )?;
        ensure!(
            initialized["serverInfo"]["name"] == "dragon-head-mcp",
            "unexpected MCP server"
        );
        process.send(json!({"jsonrpc":"2.0", "method":"notifications/initialized"}))?;
        Ok(process)
    }

    fn send(&mut self, message: Value) -> Result<()> {
        let stdin = self.stdin.as_mut().context("MCP stdin closed")?;
        writeln!(stdin, "{message}")?;
        stdin.flush()?;
        Ok(())
    }

    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))?;
        let line = self
            .responses
            .recv_timeout(timeout)
            .with_context(|| format!("waiting for MCP {method} response {id}"))??;
        let response: Value = serde_json::from_str(&line)?;
        ensure!(
            response["jsonrpc"] == "2.0" && response["id"] == id,
            "invalid MCP response framing"
        );
        ensure!(
            response.get("error").is_none(),
            "MCP {method} failed: {}",
            response["error"]
        );
        response
            .get("result")
            .cloned()
            .context("MCP result missing")
    }

    fn tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        let result = self.request(
            "tools/call",
            json!({"name":name,"arguments":arguments}),
            WAIT,
        )?;
        if let Some(is_error) = result.get("isError") {
            ensure!(
                is_error.as_bool() == Some(false),
                "MCP tool {name} returned invalid/error isError"
            );
        }
        let structured = result
            .get("structuredContent")
            .cloned()
            .context("structuredContent missing")?;
        ensure!(
            structured.is_object(),
            "MCP tool {name} structuredContent must be an object"
        );
        let fallback: Value = serde_json::from_str(
            result["content"][0]["text"]
                .as_str()
                .context("text fallback missing")?,
        )?;
        ensure!(
            structured == fallback,
            "MCP structured/text content mismatch"
        );
        Ok(structured)
    }

    fn finish(&mut self) -> Result<()> {
        self.stdin.take();
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait()? {
                ensure!(status.success(), "MCP process exited with {status}");
                return Ok(());
            }
            ensure!(Instant::now() < deadline, "MCP process did not shut down");
            thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        #[cfg(unix)]
        // SAFETY: this test created the child's distinct process group before spawn.
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        #[cfg(not(unix))]
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn element(state: &Value, name: &str) -> Result<Value> {
    state["interactive_elements"]
        .as_array()
        .context("interactive_elements missing")?
        .iter()
        .find(|node| node["name"] == name)
        .cloned()
        .with_context(|| format!("semantic element {name} missing"))
}

fn action_arguments(node: &Value, action: &str, value: Option<&str>) -> Value {
    let mut arguments =
        json!({"target_id":node["id"],"target_stable_key":node["stable_key"],"action":action});
    if let Some(value) = value {
        arguments["value"] = json!(value);
    }
    arguments
}

fn callback(address: SocketAddr, id: &str, decision: &str, secret: &str) -> Result<u16> {
    let payload = json!({"user":{"id":"demo-reviewer","username":"demo-reviewer"},
        "actions":[{"action_id":decision,"value":id}]})
    .to_string();
    let body = format!("payload={}", urlencoding::encode(&payload));
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())?;
    mac.update(format!("v0:{timestamp}:{body}").as_bytes());
    let signature = hex::encode(mac.finalize().into_bytes());
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
    stream.set_read_timeout(Some(WAIT))?;
    stream.set_write_timeout(Some(WAIT))?;
    write!(stream, "POST /slack/interactions HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: {}\r\nX-Slack-Request-Timestamp: {timestamp}\r\nX-Slack-Signature: v0={signature}\r\n\r\n{body}", body.len())?;
    stream.flush()?;
    let mut status = String::new();
    BufReader::new(stream).read_line(&mut status)?;
    status
        .split_whitespace()
        .nth(1)
        .context("HTTP status missing")?
        .parse()
        .context("invalid HTTP status")
}

fn runtime_audit(dir: &Path) -> Result<Vec<Value>> {
    let mut paths = std::fs::read_dir(dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    let mut events = Vec::new();
    for path in paths {
        if path
            .extension()
            .is_some_and(|extension| extension == "ndjson")
        {
            let body = std::fs::read_to_string(path)?;
            // The asynchronous sink may currently be appending its final record.
            // Only complete NDJSON lines are evidence; the bounded poll waits for the rest.
            for line in body
                .split_inclusive('\n')
                .filter(|line| line.ends_with('\n'))
            {
                events.push(serde_json::from_str(line)?);
            }
        }
    }
    Ok(events)
}

#[test]
#[ignore = "requires Chrome; runs the shipped MCP binary with local Slack API double"]
fn high_risk_payment_requires_signed_approval_and_records_redacted_audit() -> Result<()> {
    ensure!(
        !test_bench_support::should_skip_browser_tests(),
        "Chrome is required for the high-risk demo; set CHROME_PATH"
    );
    let services = LocalServices::start()?;
    let webhook_reservation = TcpListener::bind("127.0.0.1:0")?;
    let webhook = webhook_reservation.local_addr()?;
    let dir = tempfile::tempdir()?;
    let config_dir = dir.path().join("dragon-head");
    std::fs::create_dir_all(&config_dir)?;
    let audit_dir = dir.path().join("runtime-audit");
    let bridge_audit = dir.path().join("bridge.ndjson");
    let policy_path = dir.path().join("policy.json");
    let rule = PolicyRule {
        id: "expense-threshold".to_owned(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_owned()),
        text_regex: Some("submit expense".to_owned()),
        context_regex: None,
        action: PolicyAction::Allow,
        // Allow rules cannot declare a scope; threshold promotion defaults to action_only.
        scope: None,
        outcome_projector: Some(OutcomeProjectorConfig {
            amount_regex: Some(r"\$\s*(?P<amount>[\d,]+(?:\.\d{1,2})?)".to_owned()),
            warn_if_amount_exceeds: Some(500.0),
            block_if_amount_exceeds: None,
        }),
    };
    std::fs::write(&policy_path, serde_json::to_vec(&vec![rule])?)?;
    // JSON string quoting is valid for these generated path strings in TOML too.
    let config = format!("[navigation]\nallow_private_network = true\n[policy]\nfile = {}\n[audit]\nlog_dir = {}\nmax_bytes = 0\ndurability = \"sync\"\n[hitl_bridge]\nenabled = true\nbind_addr = \"{webhook}\"\naudit_log = {}\npoll_interval_ms = 20\nlocal_slack_api_base_url = \"http://{}/api\"\n",
        serde_json::to_string(&policy_path)?, serde_json::to_string(&audit_dir)?, serde_json::to_string(&bridge_audit)?, services.address);
    std::fs::write(config_dir.join("config.toml"), config)?;
    drop(webhook_reservation);
    let mut mcp = McpProcess::start(dir.path())?;
    ensure!(
        mcp.tool(
            "navigate",
            json!({"url":format!("http://{}/payment",services.address)})
        )?["status"]
            == "ok",
        "navigate failed"
    );
    let state = mcp.tool("get_state", json!({"format":"json","force_refresh":true}))?;
    for (name, value) in [
        ("Email", EMAIL),
        ("Password", PASSWORD),
        ("Card", CARD),
        ("Amount", "900"),
    ] {
        let input = element(&state, name)?;
        ensure!(
            mcp.tool("act", action_arguments(&input, "type", Some(value)))?["status"] == "ok",
            "typing {name} failed"
        );
    }
    let state = mcp.tool("get_state", json!({"format":"json","force_refresh":true}))?;
    let submit = element(&state, "Submit expense")?;
    let receipt = element(&state, "Payment receipt")?;
    let click = action_arguments(&submit, "click", None);
    let gated = mcp.tool("act", click.clone())?;
    ensure!(
        gated["status"] == "requires_human_approval"
            && gated["rule_id"] == "expense-threshold"
            && gated["scope"] == "action_only",
        "threshold did not require approval: {gated}"
    );
    ensure!(
        gated["outcome_projection"] == json!({"projected_amount":900.0,"risk_level":"high"}),
        "unexpected projection: {gated}"
    );
    let prompt = services.next_request("chat.postMessage")?;
    let buttons = prompt["blocks"]
        .as_array()
        .context("prompt blocks missing")?
        .iter()
        .find_map(|block| block.get("elements").and_then(Value::as_array))
        .context("approval buttons missing")?;
    let id = buttons
        .iter()
        .find(|button| button["action_id"] == "approve")
        .and_then(|button| button["value"].as_str())
        .context("approval UUID missing")?;
    ensure!(
        buttons
            .iter()
            .any(|button| button["action_id"] == "reject" && button["value"] == id),
        "reject button UUID mismatch"
    );
    ensure!(
        prompt.to_string().contains("$900.00") && prompt.to_string().contains("High"),
        "Slack prompt lacks projection"
    );
    ensure!(
        services.submissions.load(Ordering::SeqCst) == 0,
        "unapproved submit executed"
    );
    ensure!(
        callback(webhook, id, "approve", "wrong-secret")? == 401,
        "invalid signature accepted"
    );
    ensure!(
        // The bridge creates its audit log at startup (ISSUE-352), so "nothing recorded" means
        // empty rather than absent.
        std::fs::metadata(&bridge_audit).map_or(true, |meta| meta.len() == 0)
            && services.submissions.load(Ordering::SeqCst) == 0,
        "invalid signature changed approval/page"
    );
    ensure!(
        mcp.tool("act", click.clone())?["status"] == "requires_human_approval",
        "bad signature granted approval"
    );
    ensure!(
        callback(webhook, id, "approve", SECRET)? == 200,
        "signed approval failed"
    );
    let update = services.next_request("chat.update")?;
    ensure!(update["ts"] == MESSAGE_TS, "updated wrong Slack message");
    ensure!(
        update.to_string().contains("approved") && update.to_string().contains("demo-reviewer"),
        "resolution update missing reviewer"
    );
    ensure!(
        services.submissions.load(Ordering::SeqCst) == 0,
        "approval executed action without retry"
    );
    ensure!(
        mcp.tool("act", click)?["status"] == "ok",
        "approved retry failed"
    );
    let verified = mcp.tool("verify",json!({"target_id":receipt["id"],"target_stable_key":receipt["stable_key"],"expected":{"text":"Submitted 1"}}))?;
    ensure!(
        verified["matched"] == true,
        "receipt did not record exactly one submit"
    );
    let deadline = Instant::now() + WAIT;
    while services.submissions.load(Ordering::SeqCst) != 1 {
        ensure!(
            Instant::now() < deadline,
            "timed out waiting for fixture submission"
        );
        thread::sleep(Duration::from_millis(20));
    }
    ensure!(
        *services.submitted_bodies.lock().unwrap()
            == vec![json!({
                "email": EMAIL, "password": PASSWORD, "card": CARD, "amount": "900"
            })],
        "fixture submission did not contain the actual typed form values"
    );
    let events = loop {
        let events = runtime_audit(&audit_dir)?;
        if events
            .iter()
            .any(|event| event["type"] == "TOOL_CALL" && event["tool_name"] == "verify_text")
        {
            break events;
        }
        ensure!(
            Instant::now() < deadline,
            "timed out waiting for persisted audit"
        );
        thread::sleep(Duration::from_millis(20));
    };
    let tool_calls: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "TOOL_CALL")
        .collect();
    let tool_names: Vec<_> = tool_calls
        .iter()
        .filter_map(|event| event["tool_name"].as_str())
        .collect();
    ensure!(
        tool_names
            == [
                "navigate",
                "act",
                "act",
                "act",
                "act",
                "act",
                "act",
                "act",
                "verify_text"
            ],
        "unexpected persisted tool-call sequence: {tool_names:?}"
    );
    ensure!(
        tool_calls
            .first()
            .is_some_and(|event| event["tool_name"] == "navigate"),
        "navigate audit missing"
    );
    let types: Vec<_> = tool_calls
        .iter()
        .filter(|event| event["tool_name"] == "act" && event["args"]["action"] == "type")
        .collect();
    ensure!(
        types.len() == 4 && types.iter().all(|event| event["args"]["value"] == "***"),
        "form-fill audit missing/redaction failed"
    );
    let policy = events
        .iter()
        .position(|event| {
            event["type"] == "POLICY_DECISION" && event["decision"] == "require_human_approval"
        })
        .context("approval policy audit missing")?;
    let request = events
        .iter()
        .position(|event| event["type"] == "HITL_EVENT" && event["event_type"] == "request")
        .context("HITL request audit missing")?;
    ensure!(policy < request, "policy/HITL audit order incorrect");
    let records: Vec<Value> = std::fs::read_to_string(&bridge_audit)?
        .lines()
        .map(serde_json::from_str)
        .collect::<std::result::Result<_, _>>()?;
    ensure!(
        records.len() == 1
            && records[0]["id"] == id
            && records[0]["decision"] == "approved"
            && records[0]["decided_by"] == "demo-reviewer",
        "bridge resolution audit mismatch"
    );
    ensure!(
        records[0]["outcome_projection"] == gated["outcome_projection"],
        "bridge projection mismatch"
    );
    let serialized = format!(
        "{}{}",
        serde_json::to_string(&events)?,
        serde_json::to_string(&records)?
    );
    for sensitive in [EMAIL, PASSWORD, CARD] {
        ensure!(!serialized.contains(sensitive), "PII leaked in audit");
    }
    mcp.finish()?;
    eprintln!("PASS: shipped MCP + Chrome; $900 threshold blocked submit; invalid signature rejected; signed approval updated chat; explicit retry submitted once; runtime/bridge audits redacted and correlated.");
    Ok(())
}
