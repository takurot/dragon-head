//! `dragon-head-mcp --self-test` (ISSUE-191): a non-mutating local readiness check.
//!
//! Runs the `--doctor` checks, starts the same server stack as the stdio loop, then drives its
//! JSON-RPC handler through `initialize`, `tools/list` and a `get_usage_report` call so operators
//! need not hand-craft stdin lines. Only the checks themselves are produced here; `main.rs` owns
//! server construction and the exit code.

use crate::doctor::{CheckResult, DoctorReport};
use crate::protocol::LATEST_PROTOCOL_VERSION;
use crate::{McpBackend, McpServer};
use serde_json::{json, Value};

const SELF_TEST_CLIENT_NAME: &str = "dragon-head-mcp-self-test";

/// Completes the doctor `report` with a server-startup check and the JSON-RPC protocol checks.
///
/// `start` builds the real server plus a guard that must outlive the checks (e.g. the plugin
/// host's epoch driver). It is not called when the doctor checks already failed, because server
/// startup would only repeat the same failure less clearly.
pub fn run_self_test<B, G>(
    mut report: DoctorReport,
    start: impl FnOnce() -> anyhow::Result<(McpServer<B>, G)>,
) -> DoctorReport
where
    B: McpBackend,
{
    if !report.all_passed() {
        report.checks.push(startup_check(Err(
            "skipped: fix the failed checks above first".to_string(),
        )));
        return report;
    }

    match start() {
        Ok((mut server, _guard)) => {
            report
                .checks
                .push(startup_check(Ok("server started".to_string())));
            report.checks.extend(run_protocol_checks(&mut server));
        }
        Err(err) => report.checks.push(startup_check(Err(format!("{err:#}")))),
    }
    report
}

fn startup_check(outcome: Result<String, String>) -> CheckResult {
    check("MCP server startup", outcome)
}

/// Exercises `initialize`, `tools/list` and `get_usage_report` against `server`.
pub fn run_protocol_checks<B: McpBackend>(server: &mut McpServer<B>) -> Vec<CheckResult> {
    let initialize = call(
        server,
        1,
        "initialize",
        json!({
            "protocolVersion": LATEST_PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": {"name": SELF_TEST_CLIENT_NAME, "version": env!("CARGO_PKG_VERSION")}
        }),
    )
    .and_then(|result| describe_initialize(&result));

    let tools_list =
        call(server, 2, "tools/list", json!({})).and_then(|result| describe_tools(&result));

    let usage_report = call(
        server,
        3,
        "tools/call",
        json!({"name": "get_usage_report", "arguments": {}}),
    )
    .and_then(|result| describe_usage_report(&result));

    vec![
        check("initialize", initialize),
        check("tools/list", tools_list),
        check("get_usage_report", usage_report),
    ]
}

fn check(name: &'static str, outcome: Result<String, String>) -> CheckResult {
    let (passed, detail) = match outcome {
        Ok(detail) => (true, detail),
        Err(detail) => (false, detail),
    };
    CheckResult {
        name,
        passed,
        informational: false,
        detail,
    }
}

fn call<B: McpBackend>(
    server: &mut McpServer<B>,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    let raw = server
        .handle_jsonrpc(&request.to_string())
        .ok_or_else(|| format!("no response to {method}"))?;
    parse_result(&raw)
}

fn parse_result(raw: &str) -> Result<Value, String> {
    let response: Value =
        serde_json::from_str(raw).map_err(|err| format!("invalid JSON-RPC response: {err}"))?;
    if let Some(error) = response.get("error") {
        return Err(format!("JSON-RPC error: {error}"));
    }
    response
        .get("result")
        .cloned()
        .ok_or_else(|| "response has neither result nor error".to_string())
}

fn str_field<'a>(value: &'a Value, path: &[&str]) -> Result<&'a str, String> {
    let mut current = value;
    for key in path {
        current = current
            .get(key)
            .ok_or_else(|| format!("missing field {}", path.join(".")))?;
    }
    current
        .as_str()
        .ok_or_else(|| format!("field {} is not a string", path.join(".")))
}

fn describe_initialize(result: &Value) -> Result<String, String> {
    let name = str_field(result, &["serverInfo", "name"])?;
    let version = str_field(result, &["serverInfo", "version"])?;
    let protocol = str_field(result, &["protocolVersion"])?;
    Ok(format!("server {name} {version}, protocol {protocol}"))
}

fn describe_tools(result: &Value) -> Result<String, String> {
    let tools = result
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing field tools".to_string())?;
    let names = tools
        .iter()
        .map(|tool| str_field(tool, &["name"]))
        .collect::<Result<Vec<_>, _>>()?;
    if !names.contains(&"get_usage_report") {
        return Err(format!(
            "get_usage_report is not advertised (tools: {})",
            names.join(", ")
        ));
    }
    Ok(format!("{} tools ({})", names.len(), names.join(", ")))
}

fn describe_usage_report(result: &Value) -> Result<String, String> {
    let plan = str_field(result, &["structuredContent", "plan_tier"])?;
    Ok(format!("plan tier {plan}"))
}

/// `--self-test` runs before (and instead of) the stdio JSON-RPC session, so writing to stdout
/// here is the intended human-facing CLI output, not a protocol-corruption risk (ISSUE-254).
#[allow(clippy::print_stdout)]
pub fn print_report(report: &DoctorReport) {
    println!("dragon-head-mcp self-test");
    for check in &report.checks {
        let icon = if !check.passed {
            "✗"
        } else if check.informational {
            "ℹ"
        } else {
            "✓"
        };
        println!("  {icon} {}: {}", check.name, check.detail);
    }
    let verdict = if report.all_passed() { "PASS" } else { "FAIL" };
    println!("\nSelf-test: {verdict}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    struct StubBackend;

    impl McpBackend for StubBackend {
        fn navigate(&mut self, _: Value) -> anyhow::Result<Value> {
            Ok(json!({}))
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

    fn passing_doctor() -> DoctorReport {
        DoctorReport {
            checks: vec![check("Chrome/Chromium", Ok("found".to_string()))],
        }
    }

    fn failing_doctor() -> DoctorReport {
        DoctorReport {
            checks: vec![check("Chrome/Chromium", Err("not found".to_string()))],
        }
    }

    #[test]
    fn protocol_checks_pass_and_report_version_protocol_tools_and_plan() {
        let mut server = McpServer::new(StubBackend);
        let tool_count = server.tools().len();

        let checks = run_protocol_checks(&mut server);

        assert_eq!(
            checks.iter().map(|c| c.name).collect::<Vec<_>>(),
            ["initialize", "tools/list", "get_usage_report"]
        );
        assert!(checks.iter().all(|c| c.passed), "{checks:?}");
        assert_eq!(
            checks[0].detail,
            format!(
                "server dragon-head-mcp {}, protocol {LATEST_PROTOCOL_VERSION}",
                env!("CARGO_PKG_VERSION")
            )
        );
        assert!(
            checks[1]
                .detail
                .starts_with(&format!("{tool_count} tools (")),
            "{}",
            checks[1].detail
        );
        assert!(checks[1].detail.contains("get_usage_report"));
        assert_eq!(checks[2].detail, "plan tier enterprise");
    }

    #[test]
    fn run_self_test_appends_startup_and_protocol_checks_after_doctor() {
        let report = run_self_test(passing_doctor(), || Ok((McpServer::new(StubBackend), ())));

        assert!(report.all_passed(), "{report:?}");
        assert_eq!(
            report.checks.iter().map(|c| c.name).collect::<Vec<_>>(),
            [
                "Chrome/Chromium",
                "MCP server startup",
                "initialize",
                "tools/list",
                "get_usage_report"
            ]
        );
    }

    #[test]
    fn run_self_test_fails_with_startup_error_detail() {
        let report = run_self_test(passing_doctor(), || {
            Err::<(McpServer<StubBackend>, ()), _>(anyhow!("chrome launch failed"))
        });

        assert!(!report.all_passed());
        let startup = report.checks.last().unwrap();
        assert_eq!(startup.name, "MCP server startup");
        assert!(!startup.passed);
        assert!(startup.detail.contains("chrome launch failed"));
    }

    #[test]
    fn run_self_test_skips_startup_when_doctor_failed() {
        let report = run_self_test(
            failing_doctor(),
            || -> anyhow::Result<(McpServer<StubBackend>, ())> {
                panic!("server must not be started when doctor checks failed")
            },
        );

        assert!(!report.all_passed());
        let startup = report.checks.last().unwrap();
        assert_eq!(startup.name, "MCP server startup");
        assert!(startup.detail.starts_with("skipped"));
    }

    #[test]
    fn call_surfaces_jsonrpc_errors_and_failed_check_fails_the_report() {
        let mut server = McpServer::new(StubBackend);

        let err = call(&mut server, 1, "bogus/method", json!({})).unwrap_err();
        assert!(err.contains("JSON-RPC error"), "{err}");

        let mut checks = run_protocol_checks(&mut server);
        checks.push(check("bogus/method", Err(err)));
        assert!(!DoctorReport { checks }.all_passed());
    }

    #[test]
    fn parse_result_rejects_errors_garbage_and_empty_responses() {
        assert!(parse_result("not json").unwrap_err().contains("invalid"));
        assert!(parse_result(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"nope"}}"#
        )
        .unwrap_err()
        .contains("JSON-RPC error"));
        assert!(parse_result(r#"{"jsonrpc":"2.0","id":1}"#)
            .unwrap_err()
            .contains("neither"));
        assert_eq!(
            parse_result(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#).unwrap(),
            json!({"ok": true})
        );
    }

    #[test]
    fn describe_tools_fails_when_usage_report_tool_is_missing() {
        let err = describe_tools(&json!({"tools": [{"name": "navigate"}]})).unwrap_err();
        assert!(err.contains("get_usage_report is not advertised"), "{err}");
    }

    #[test]
    fn describe_functions_fail_on_malformed_results() {
        assert!(describe_initialize(&json!({"protocolVersion": "x"})).is_err());
        assert!(describe_tools(&json!({})).is_err());
        assert!(describe_usage_report(&json!({"structuredContent": {}})).is_err());
    }
}
