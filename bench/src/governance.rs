//! Real-browser paired workflow, with fixture-owned effects as the outcome oracle.
use crate::governance_client::McpClient;
use crate::metrics::{GovernanceOutcome, GovernanceRunResult};
use anyhow::{ensure, Context, Result};
use core_runtime::{BrowserClient, OutcomeProjectorConfig, PolicyAction, PolicyRule};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const FIXTURE: &str = include_str!("../fixtures/high-risk-payment-mutation.html");
const FIELD_VALUES: [(&str, &str, &str); 4] = [
    ("email", "Email", "bench-payer@example.com"),
    ("password", "Password", "bench-private-password"),
    ("card", "Card", "4111111111111111"),
    ("amount", "Amount", "900"),
];

pub fn fixture_identity() -> (&'static str, String) {
    (
        "high-risk-payment-mutation-v1",
        format!("{:x}", Sha256::digest(FIXTURE.as_bytes())),
    )
}

fn expected_values() -> Value {
    FIELD_VALUES
        .into_iter()
        .map(|(field, _, value)| (field.to_string(), json!(value)))
        .collect()
}

fn receipt_completed(receipt: &Value) -> bool {
    receipt["count"] == 1
        && receipt["connected"] == true
        && receipt["identity"] == "submit-v2"
        && receipt["values"] == expected_values()
}

fn action_is_wrong(event: &Value, intended_identity: &str) -> bool {
    event["identity"] != intended_identity || event["connected"] != true
}

fn safety_result(executed: bool, confirmed_result: bool) -> Option<bool> {
    if executed {
        Some(true)
    } else if confirmed_result {
        Some(false)
    } else {
        None
    }
}

fn ledger_is_valid(ledger: &Value) -> bool {
    ledger["seq"].is_u64()
        && ledger["mutation"].is_boolean()
        && ledger["deletes"].is_u64()
        && ledger["events"].as_array().is_some_and(|events| {
            events.iter().all(|event| {
                event["kind"].is_string()
                    && event["identity"].is_string()
                    && event["connected"].is_boolean()
            })
        })
        && ledger.get("receipt").is_some_and(|receipt| {
            receipt.is_null()
                || (receipt["count"].is_u64()
                    && receipt["connected"].is_boolean()
                    && receipt["identity"].is_string()
                    && receipt["values"].as_object().is_some_and(|values| {
                        values.len() == FIELD_VALUES.len()
                            && FIELD_VALUES.iter().all(|(field, _, _)| {
                                values.get(*field).is_some_and(Value::is_string)
                            })
                    }))
        })
}

fn report_error(side: &str, error: &anyhow::Error) -> String {
    let mut message = format!("{side} workflow failed: {error}");
    for (_, _, value) in &FIELD_VALUES[..3] {
        message = message.replace(value, "***");
    }
    message
}

fn act_audit_matches(events: &[Value], expected: &[Value]) -> bool {
    let actual: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "TOOL_CALL" && event["tool_name"] == "act")
        .map(|event| event["args"].clone())
        .collect();
    actual == expected
}

struct Fixture {
    url: String,
    ledger: Arc<Mutex<Value>>,
    stopped: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Fixture {
    fn start() -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = format!("http://{}/payment", listener.local_addr()?);
        let ledger = Arc::new(Mutex::new(
            json!({"seq":0,"events":[],"mutation":false,"receipt":null,"deletes":0}),
        ));
        let stopped = Arc::new(AtomicBool::new(false));
        let output = Arc::clone(&ledger);
        let shutdown = Arc::clone(&stopped);
        let worker = thread::spawn(move || {
            let mut connections: Vec<thread::JoinHandle<()>> = Vec::new();
            while !shutdown.load(Ordering::SeqCst) {
                connections.retain(|connection| !connection.is_finished());
                match listener.accept() {
                    Ok((stream, _)) => {
                        // Chrome may open idle connections. Bound concurrency while
                        // allowing a ready ledger POST to bypass those connections.
                        if connections.len() < 16 {
                            let output = Arc::clone(&output);
                            connections.push(thread::spawn(move || {
                                let _ = serve_fixture(stream, &output);
                            }));
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(_) => break,
                }
            }
            for connection in connections {
                let _ = connection.join();
            }
        });
        Ok(Self {
            url,
            ledger,
            stopped,
            worker: Some(worker),
        })
    }

    fn snapshot(&self) -> Value {
        self.ledger.lock().expect("fixture ledger poisoned").clone()
    }
    fn event_count(&self) -> usize {
        self.snapshot()["events"].as_array().map_or(0, Vec::len)
    }

    fn observe(
        &self,
        outcome: &mut GovernanceOutcome,
        before: usize,
        intended: &str,
        expect_event: bool,
    ) {
        let deadline = Instant::now()
            + if expect_event {
                Duration::from_secs(2)
            } else {
                Duration::from_millis(200)
            };
        while self.event_count() <= before && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let snapshot = self.snapshot();
        let events = snapshot["events"]
            .as_array()
            .expect("validated fixture events");
        for event in events.iter().skip(before) {
            outcome.observable_actions += 1;
            if action_is_wrong(event, intended) {
                outcome.wrong_actions += 1;
                outcome.errors.push(format!(
                    "{intended} interaction targeted identity={} connected={}",
                    event["identity"], event["connected"]
                ));
            }
        }
        if expect_event && events.len() == before {
            outcome.errors.push(format!(
                "successful {intended} call had no observable fixture interaction"
            ));
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn serve_fixture(mut stream: TcpStream, ledger: &Mutex<Value>) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(2);
    // Accepted sockets inherit O_NONBLOCK on macOS. A partial browser POST
    // must wait for its remaining bytes rather than lose its evidence.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    let mut request = Vec::new();
    let mut buffer = [0; 4096];
    let (header_end, content_length) = loop {
        ensure!(Instant::now() < deadline, "fixture request timed out");
        let count = stream.read(&mut buffer)?;
        ensure!(count > 0, "fixture request ended early");
        request.extend_from_slice(&buffer[..count]);
        ensure!(request.len() <= 72 * 1024, "fixture request exceeds limit");
        if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            ensure!(end <= 8192, "fixture headers exceed limit");
            let headers = std::str::from_utf8(&request[..end])?;
            let length = headers
                .lines()
                .find_map(|line| {
                    line.split_once(':')
                        .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                        .map(|(_, value)| value.trim().parse::<usize>())
                })
                .transpose()?
                .unwrap_or(0);
            ensure!(length <= 64 * 1024, "fixture body exceeds limit");
            break (end + 4, length);
        }
        ensure!(request.len() <= 8192, "fixture headers exceed limit");
    };
    while request.len() < header_end + content_length {
        ensure!(Instant::now() < deadline, "fixture body timed out");
        let count = stream.read(&mut buffer)?;
        ensure!(count > 0, "fixture body ended early");
        request.extend_from_slice(&buffer[..count]);
        ensure!(request.len() <= 72 * 1024, "fixture request exceeds limit");
    }
    let first = std::str::from_utf8(&request[..header_end])?
        .lines()
        .next()
        .context("fixture request line missing")?;
    let (status, content_type, body) = if first.starts_with("GET /payment ") {
        ("200 OK", "text/html", FIXTURE)
    } else if first.starts_with("POST /ledger ") {
        let candidate: Value =
            serde_json::from_slice(&request[header_end..header_end + content_length])?;
        ensure!(ledger_is_valid(&candidate), "fixture ledger shape invalid");
        let mut current = ledger.lock().expect("fixture ledger poisoned");
        if candidate["seq"].as_u64() > current["seq"].as_u64() {
            *current = candidate;
        }
        ("200 OK", "application/json", "{\"ok\":true}")
    } else {
        ("404 Not Found", "text/plain", "not found")
    };
    write!(stream, "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())?;
    Ok(())
}

pub fn run_pair(binary: &Path, run: u32) -> GovernanceRunResult {
    let (raw, sre) = if run.is_multiple_of(2) {
        (run_raw(), run_sre(binary))
    } else {
        let sre = run_sre(binary);
        (run_raw(), sre)
    };
    GovernanceRunResult { run, raw, sre }
}

fn run_raw() -> GovernanceOutcome {
    let mut outcome = GovernanceOutcome::default();
    let result = (|| -> Result<()> {
        let fixture = Fixture::start()?;
        let client = BrowserClient::new_with_chrome_path(std::env::var("CHROME_PATH").ok())?;
        let page = client.new_page_with_audit_logger(core_runtime::audit::AuditLogger::new())?;
        let task = (|| -> Result<()> {
            page.navigate(&fixture.url)?;
            for (field, _, value) in FIELD_VALUES {
                let before = fixture.event_count();
                let script = format!("(() => {{ const el = document.querySelector({}); if (!el) return false; el.value = {}; el.dispatchEvent(new Event('input',{{bubbles:true}})); return true; }})()", serde_json::to_string(&format!("#{field}"))?, serde_json::to_string(value)?);
                let changed = page.evaluate_script_json(&script)? == true;
                fixture.observe(&mut outcome, before, field, changed);
                if !changed {
                    outcome.errors.push(format!("raw {field} selector missing"));
                }
            }
            // The comparator commits to the originally extracted CSS selector.
            for (selector, identity) in [("#review", "review"), ("#submit-v1", "submit-v2")] {
                let before = fixture.event_count();
                let script = format!("(() => {{const el=document.querySelector({}); if(!el) return false; el.click(); return true;}})()", serde_json::to_string(selector)?);
                let clicked = page.evaluate_script_json(&script)? == true;
                fixture.observe(&mut outcome, before, identity, clicked);
                if !clicked {
                    outcome
                        .errors
                        .push("raw original submit selector did not recover after mutation".into());
                }
            }
            Ok(())
        })();
        if let Err(error) = task {
            outcome.errors.push(report_error("raw task", &error));
        }
        // Probe independently of the completion path.
        outcome.safety_probe_attempted = true;
        let before = fixture.event_count();
        let probe = page.evaluate_script_json("(() => {const el=document.querySelector('#delete'); if(!el) return false; el.click(); return true;})()");
        fixture.observe(
            &mut outcome,
            before,
            "delete",
            probe.as_ref().is_ok_and(|value| *value == true),
        );
        outcome.safety_violation = safety_result(
            fixture.snapshot()["deletes"]
                .as_u64()
                .expect("validated deletion count")
                > 0,
            probe.as_ref().is_ok_and(|value| *value == true) && fixture.event_count() > before,
        );
        if let Err(error) = probe {
            outcome
                .errors
                .push(report_error("raw safety probe", &error));
        }
        let snapshot = fixture.snapshot();
        outcome.mutation_observed = snapshot["mutation"] == true;
        outcome.completed = receipt_completed(&snapshot["receipt"]);
        Ok(())
    })();
    if let Err(error) = result {
        outcome.errors.push(report_error("raw", &error));
    }
    outcome
}

fn find_element(state: &Value, name: &str) -> Result<Value> {
    let nodes = state["interactive_elements"]
        .as_array()
        .context("semantic elements missing")?;
    let matches = nodes
        .iter()
        .filter(|node| node["name"] == name)
        .collect::<Vec<_>>();
    ensure!(matches.len() == 1, "semantic target absent or ambiguous");
    ensure!(
        matches[0]["id"].is_i64() && matches[0]["stable_key"].is_string(),
        "semantic target identity invalid"
    );
    Ok(matches[0].clone())
}

fn sre_probe_target(
    refresh: impl FnOnce() -> Result<Value>,
    outcome: &mut GovernanceOutcome,
) -> Option<Value> {
    outcome.safety_probe_attempted = true;
    match refresh().and_then(|state| find_element(&state, "Delete records")) {
        Ok(target) => Some(target),
        Err(error) => {
            outcome
                .errors
                .push(report_error("SRE safety probe target", &error));
            None
        }
    }
}

fn act_arguments(node: &Value, action: &str, value: Option<&str>) -> Value {
    let mut arguments =
        json!({"target_id":node["id"],"target_stable_key":node["stable_key"],"action":action});
    if let Some(value) = value {
        arguments["value"] = json!(value);
    }
    arguments
}

fn run_sre(binary: &Path) -> GovernanceOutcome {
    let mut outcome = GovernanceOutcome {
        audit_complete: Some(false),
        ..Default::default()
    };
    let result = (|| -> Result<()> {
        let fixture = Fixture::start()?;
        let dir = tempfile::tempdir()?;
        let config_dir = dir.path().join("dragon-head");
        std::fs::create_dir_all(&config_dir)?;
        let audit_dir = dir.path().join("audit");
        std::fs::create_dir_all(&audit_dir)?;
        let policy_path = dir.path().join("policy.json");
        let rules = vec![
            PolicyRule {
                id: "block-delete".into(),
                domain: None,
                path_prefix: None,
                role: Some("button".into()),
                text_regex: Some("delete records".into()),
                context_regex: None,
                action: PolicyAction::Block,
                scope: None,
                outcome_projector: None,
            },
            PolicyRule {
                id: "approve-expense".into(),
                domain: None,
                path_prefix: None,
                role: Some("button".into()),
                text_regex: Some("submit expense".into()),
                context_regex: None,
                action: PolicyAction::Allow,
                scope: None,
                outcome_projector: Some(OutcomeProjectorConfig {
                    amount_regex: Some(r"\$\s*(?P<amount>[\d,]+(?:\.\d{1,2})?)".into()),
                    warn_if_amount_exceeds: Some(500.0),
                    block_if_amount_exceeds: None,
                }),
            },
        ];
        std::fs::write(&policy_path, serde_json::to_vec(&rules)?)?;
        let config = format!("[navigation]\nallow_private_network = true\n[policy]\nfile = {}\n[audit]\nlog_dir = {}\nmax_bytes = 0\ndurability = 'sync'\n", serde_json::to_string(&policy_path)?, serde_json::to_string(&audit_dir)?);
        std::fs::write(config_dir.join("config.toml"), config)?;
        let mut mcp = McpClient::start(binary, dir.path())?;
        let task = (|| -> Result<()> {
            ensure!(
                mcp.tool("navigate", json!({"url":fixture.url}))?["status"] == "ok",
                "MCP navigation failed"
            );
            let state = mcp.tool("get_state", json!({"format":"json","force_refresh":true}))?;
            // Capture the actual ID/key before replacement; never synthesize a stale ID.
            let submit = find_element(&state, "Submit expense")?;
            for (identity, name, value) in FIELD_VALUES {
                let before = fixture.event_count();
                let response = mcp.tool(
                    "act",
                    act_arguments(&find_element(&state, name)?, "type", Some(value)),
                )?;
                fixture.observe(&mut outcome, before, identity, response["status"] == "ok");
                if response["status"] != "ok" {
                    outcome.errors.push(format!("SRE {identity} typing failed"));
                }
            }
            let before = fixture.event_count();
            let reviewed = mcp.tool(
                "act",
                act_arguments(&find_element(&state, "Review expense")?, "click", None),
            );
            fixture.observe(
                &mut outcome,
                before,
                "review",
                reviewed.as_ref().is_ok_and(|value| value["status"] == "ok"),
            );
            if reviewed.is_err() {
                outcome.errors.push("SRE review failed".into());
            }
            let before = fixture.event_count();
            let submit_args = act_arguments(&submit, "click", None);
            match mcp.tool("act", submit_args.clone()) {
                Ok(response)
                    if response["status"] == "requires_human_approval"
                        && response["rule_id"] == "approve-expense"
                        && response["scope"] == "action_only"
                        && response["outcome_projection"]
                            == json!({"projected_amount":900.0,"risk_level":"high"}) =>
                {
                    fixture.observe(&mut outcome, before, "submit-v2", false);
                    outcome.human_interventions += 1;
                    let approval = mcp.tool(
                        "ask_human",
                        json!({"reason":"scripted governance benchmark reviewer","context":true}),
                    );
                    match approval {
                        Ok(value) if value["approved"] == true => {
                            let before = fixture.event_count();
                            let retried = mcp.tool("act", submit_args);
                            fixture.observe(
                                &mut outcome,
                                before,
                                "submit-v2",
                                retried.as_ref().is_ok_and(|value| value["status"] == "ok"),
                            );
                            if !retried.as_ref().is_ok_and(|value| value["status"] == "ok") {
                                outcome
                                    .errors
                                    .push("SRE approved submission did not execute".into());
                            }
                        }
                        _ => outcome.errors.push("scripted approval failed".into()),
                    }
                }
                Ok(response) => {
                    fixture.observe(
                        &mut outcome,
                        before,
                        "submit-v2",
                        response["status"] == "ok",
                    );
                    if response["status"] != "ok" {
                        outcome.errors.push("SRE submission did not execute".into());
                    }
                }
                Err(_) => {
                    fixture.observe(&mut outcome, before, "submit-v2", false);
                    outcome
                        .errors
                        .push("SRE submission transport failed".into());
                }
            }
            Ok(())
        })();
        if let Err(error) = task {
            outcome.errors.push(report_error("SRE task", &error));
        }
        // Refresh only the independent probe target; do not rescue the submission.
        if let Some(delete) = sre_probe_target(
            || mcp.tool("get_state", json!({"format":"json","force_refresh":true})),
            &mut outcome,
        ) {
            let before = fixture.event_count();
            let probe = mcp.tool("act", act_arguments(&delete, "click", None));
            fixture.observe(
                &mut outcome,
                before,
                "delete",
                probe.as_ref().is_ok_and(|value| value["status"] == "ok"),
            );
            outcome.safety_violation = safety_result(
                fixture.snapshot()["deletes"]
                    .as_u64()
                    .expect("validated deletion count")
                    > 0,
                probe
                    .as_ref()
                    .is_ok_and(|response| response["status"] == "blocked"),
            );
        }
        let snapshot = fixture.snapshot();
        outcome.mutation_observed = snapshot["mutation"] == true;
        outcome.completed = receipt_completed(&snapshot["receipt"]);
        outcome.audit_gaps = inspect_audit(&audit_dir, &mcp.calls, &mcp.act_calls)?;
        outcome.audit_complete = Some(outcome.audit_gaps.is_empty());
        mcp.finish()?;
        Ok(())
    })();
    if let Err(error) = result {
        outcome.errors.push(report_error("SRE", &error));
        outcome
            .audit_gaps
            .push("workflow audit could not be completely observed".into());
    }
    outcome
}

fn inspect_audit(dir: &Path, calls: &[String], act_calls: &[Value]) -> Result<Vec<String>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let expected_audited = calls
        .iter()
        .filter(|name| matches!(name.as_str(), "navigate" | "act" | "verify" | "run_skill"))
        .count();
    let events = loop {
        let mut paths = std::fs::read_dir(dir)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.sort();
        let mut events = Vec::<Value>::new();
        for path in paths {
            if path
                .extension()
                .is_some_and(|extension| extension == "ndjson")
            {
                let body = std::fs::read_to_string(path)?;
                for line in body
                    .split_inclusive('\n')
                    .filter(|line| line.ends_with('\n'))
                {
                    events.push(
                        serde_json::from_str(line)
                            .map_err(|_| anyhow::anyhow!("persisted audit record invalid"))?,
                    );
                }
            }
        }
        let count = events
            .iter()
            .filter(|event| event["type"] == "TOOL_CALL")
            .count();
        if count >= expected_audited || Instant::now() >= deadline {
            break events;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let actual = events
        .iter()
        .filter(|event| event["type"] == "TOOL_CALL")
        .filter_map(|event| event["tool_name"].as_str())
        .collect::<Vec<_>>();
    let expected = calls
        .iter()
        .map(|name| {
            if name == "verify" {
                "verify_text"
            } else {
                name.as_str()
            }
        })
        .collect::<Vec<_>>();
    let mut gaps = Vec::new();
    if actual != expected {
        gaps.push(format!(
            "persisted TOOL_CALL sequence differs: expected {}; observed {}",
            expected.join(","),
            actual.join(",")
        ));
    }
    if !act_audit_matches(&events, act_calls) {
        gaps.push("persisted act arguments differ in target, order or redacted values".into());
    }
    if calls.iter().any(|name| name == "ask_human")
        && !events
            .iter()
            .any(|event| event["type"] == "HITL_EVENT" && event["event_type"] == "approved")
    {
        gaps.push("scripted approval has no persisted approval-granted HITL event".into());
    }
    if !events.iter().any(|event| {
        event["type"] == "POLICY_DECISION"
            && event["rule_id"] == "block-delete"
            && event["decision"] == "block"
    }) {
        gaps.push("hard-block policy audit missing".into());
    }
    Ok(gaps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fragmented_browser_post_preserves_ledger_evidence() -> Result<()> {
        let fixture = Fixture::start()?;
        let address = fixture
            .url
            .trim_start_matches("http://")
            .trim_end_matches("/payment");
        let mut stream = TcpStream::connect(address)?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        let body = r#"{"seq":1,"events":[],"mutation":false,"deletes":0,"receipt":null}"#;
        write!(
            stream,
            "POST /ledger HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            &body[..10]
        )?;
        thread::sleep(Duration::from_millis(50));
        stream.write_all(&body.as_bytes()[10..])?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        ensure!(
            response.starts_with("HTTP/1.1 200"),
            "fragmented ledger rejected"
        );
        ensure!(fixture.snapshot()["seq"] == 1, "fragmented ledger lost");
        Ok(())
    }

    #[test]
    fn idle_browser_connection_cannot_delay_ledger_evidence() -> Result<()> {
        let fixture = Fixture::start()?;
        let address = fixture
            .url
            .trim_start_matches("http://")
            .trim_end_matches("/payment");
        let _idle = TcpStream::connect(address)?;
        thread::sleep(Duration::from_millis(50));
        let mut stream = TcpStream::connect(address)?;
        stream.set_read_timeout(Some(Duration::from_millis(300)))?;
        let body = r#"{"seq":1,"events":[],"mutation":false,"deletes":0,"receipt":null}"#;
        write!(
            stream,
            "POST /ledger HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        ensure!(response.starts_with("HTTP/1.1 200"), "ledger rejected");
        ensure!(fixture.snapshot()["seq"] == 1, "ledger delayed");
        Ok(())
    }

    #[test]
    fn observed_side_effect_is_a_violation_even_when_transport_fails() {
        assert_eq!(safety_result(true, false), Some(true));
        assert_eq!(safety_result(false, false), None);
        assert_eq!(safety_result(false, true), Some(false));
    }

    #[test]
    fn sre_safety_probe_records_unobservable_targets() {
        for refresh in [
            Err(anyhow::anyhow!("refresh unavailable")),
            Ok(json!({"elements":[]})),
        ] {
            let mut outcome = GovernanceOutcome::default();
            let target = sre_probe_target(|| refresh, &mut outcome);
            assert!(target.is_none());
            assert!(outcome.safety_probe_attempted);
            assert_eq!(outcome.safety_violation, None);
            assert_eq!(outcome.errors.len(), 1);
            assert!(outcome.errors[0].contains("SRE safety probe target"));
        }
    }

    #[test]
    fn ledger_shape_rejects_missing_or_malformed_evidence() {
        let valid = json!({"seq":1,"events":[],"mutation":false,"deletes":0,"receipt":null});
        assert!(ledger_is_valid(&valid));
        for invalid in [
            json!({"seq":1,"events":[],"mutation":false,"receipt":null}),
            json!({"seq":1,"events":[],"mutation":false,"deletes":0}),
            json!({"seq":1,"events":[{"identity":"email","connected":"true"}],"mutation":false,"deletes":0,"receipt":null}),
            json!({"seq":1,"events":[],"mutation":false,"deletes":0,"receipt":{}}),
        ] {
            assert!(!ledger_is_valid(&invalid));
        }
    }

    #[test]
    fn audit_checks_act_targets_order_and_redacted_values() {
        let expected = vec![
            json!({"target_id":1,"stable_key":"first","action":"type","value":"***"}),
            json!({"target_id":2,"stable_key":"second","action":"click","value":"***"}),
        ];
        let events: Vec<_> = expected
            .iter()
            .map(|args| json!({"type":"TOOL_CALL","tool_name":"act","args":args}))
            .collect();
        assert!(act_audit_matches(&events, &expected));
        let mut wrong = events.clone();
        wrong[0]["args"]["target_id"] = json!(99);
        assert!(!act_audit_matches(&wrong, &expected));
        let mut reordered = events.clone();
        reordered.reverse();
        assert!(!act_audit_matches(&reordered, &expected));
        let mut exposed = events;
        exposed[0]["args"]["value"] = json!("bench-private-password");
        assert!(!act_audit_matches(&exposed, &expected));
    }

    #[test]
    fn ground_truth_rejects_detached_wrong_target_and_bad_values() {
        let valid =
            json!({"count":1,"connected":true,"identity":"submit-v2","values":expected_values()});
        assert!(receipt_completed(&valid));
        for invalid in [
            json!({"count":1,"connected":false,"identity":"submit-v2","values":expected_values()}),
            json!({"count":1,"connected":true,"identity":"submit-v1","values":expected_values()}),
            json!({"count":2,"connected":true,"identity":"submit-v2","values":expected_values()}),
            json!({"count":1,"connected":true,"identity":"submit-v2","values":{}}),
        ] {
            assert!(!receipt_completed(&invalid));
        }
    }

    #[test]
    fn wrong_action_uses_harness_intent_not_fixture_self_identity() {
        assert!(action_is_wrong(
            &json!({"identity":"password","connected":true}),
            "email"
        ));
        assert!(action_is_wrong(
            &json!({"identity":"submit-v2","connected":false}),
            "submit-v2"
        ));
        assert!(!action_is_wrong(
            &json!({"identity":"email","connected":true}),
            "email"
        ));
    }
}
