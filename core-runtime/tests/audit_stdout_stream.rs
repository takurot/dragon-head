/// Regression test for ISSUE-199: `AUDIT_LOG_STDOUT` must never write to the
/// real stdout stream, because `dragon-head-mcp` uses stdout for JSON-RPC
/// framing. Writing audit events there corrupts the protocol stream. The
/// events must still reach stderr, so the feature keeps working.
///
/// The audit worker runs on a background thread, and Rust's default test
/// harness silently redirects `println!`/`eprintln!` output from threads
/// spawned during a test into its own in-memory capture buffer — so a plain
/// in-process assertion cannot observe which real OS stream the bytes land
/// on. To test the actual fix, this re-executes the compiled test binary as
/// a child process filtered to just `audit_log_stdout_probe` with
/// `--nocapture`, so the child's real stdout/stderr file descriptors (which
/// `Command::output()` pipes independently) reflect exactly what the audit
/// worker thread wrote.
use core_runtime::audit::{AuditEvent, AuditLogger};
use std::process::Command;
use std::time::Duration;

const CHILD_MARKER_ENV: &str = "DRAGON_HEAD_AUDIT_STDOUT_PROBE_CHILD";

fn tool_call(n: u32) -> AuditEvent {
    AuditEvent::ToolCall {
        tool_name: format!("tool_{n}"),
        args: serde_json::json!({ "n": n }),
        timestamp: n as u64,
    }
}

/// Child-process body: enables AUDIT_LOG_STDOUT purely through the lookup
/// closure passed to `from_env_with` (never through the real environment) and
/// logs one event. Only runs when re-exec'd with `CHILD_MARKER_ENV` set.
#[test]
fn audit_log_stdout_probe() {
    if std::env::var(CHILD_MARKER_ENV).is_err() {
        return; // Not the child invocation — no-op under a normal test run.
    }

    let logger = AuditLogger::from_env_with(|key| (key == "AUDIT_LOG_STDOUT").then(|| "1".into()));
    logger.log(tool_call(1));
    std::thread::sleep(Duration::from_millis(300));
    drop(logger);
}

#[test]
fn audit_log_stdout_env_routes_to_stderr_not_real_stdout() {
    let exe = std::env::current_exe().expect("test binary path");
    let output = Command::new(exe)
        .args(["audit_log_stdout_probe", "--exact", "--nocapture"])
        .env(CHILD_MARKER_ENV, "1")
        .output()
        .expect("failed to spawn probe child process");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !stdout.contains("[AUDIT]"),
        "AUDIT_LOG_STDOUT must never write to real stdout (JSON-RPC framing \
         channel); captured stdout: {stdout:?}"
    );
    assert!(
        stderr.contains("[AUDIT]") && stderr.contains("tool_1"),
        "AUDIT_LOG_STDOUT (via the injected lookup closure passed to \
         from_env_with) must still emit events, just on stderr; captured \
         stderr: {stderr:?}"
    );
}
