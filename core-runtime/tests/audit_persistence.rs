/// Integration tests for persistent audit sinks (PR-24 / ISSUE-09).
///
/// Exit Criteria:
/// - Events written via `AuditLogger::with_sinks` are persisted to disk.
/// - Zero event loss under high-load burst (200 events).
/// - File content is valid NDJSON.
/// - `MCP audit_retention_snapshot` metrics can reflect persistent sink stats.
use core_runtime::{
    audit::{AuditEvent, AuditLogger},
    audit_sink::{AuditSink, MeteredSink, RollingFileSink},
};
use serde_json::{json, Value};
use std::{fs, time::Duration};
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn tool_call(n: u32) -> AuditEvent {
    AuditEvent::ToolCall {
        tool_name: format!("tool_{n}"),
        args: json!({ "n": n }),
        timestamp: n as u64,
    }
}

fn policy_event(n: u32) -> AuditEvent {
    AuditEvent::PolicyDecision {
        rule_id: format!("R{n}"),
        action: "click".into(),
        decision: "allow".into(),
        timestamp: n as u64,
    }
}

fn read_all_ndjson_lines(dir: &std::path::Path) -> Vec<Value> {
    let mut lines = Vec::new();
    for entry in fs::read_dir(dir).unwrap().filter_map(|e| e.ok()) {
        let content = fs::read_to_string(entry.path()).unwrap_or_default();
        for line in content.lines() {
            if !line.is_empty() {
                lines.push(serde_json::from_str(line).expect("valid JSON line"));
            }
        }
    }
    lines
}

fn wait_for_sink_events(
    sink_dir: &std::path::Path,
    expected: usize,
    timeout: Duration,
) -> Vec<Value> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let lines = read_all_ndjson_lines(sink_dir);
        if lines.len() >= expected || std::time::Instant::now() >= deadline {
            return lines;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Events logged via `with_sinks` are written to the rolling file.
#[test]
fn audit_logger_with_rolling_file_sink_persists_events() {
    let dir = tempdir().unwrap();
    let sink = RollingFileSink::new(dir.path(), "audit", 0).unwrap();
    let logger = AuditLogger::with_sinks(vec![Box::new(sink)]);

    logger.log(tool_call(1));
    logger.log(policy_event(2));

    let lines = wait_for_sink_events(dir.path(), 2, Duration::from_secs(2));
    assert_eq!(lines.len(), 2, "both events must be persisted");
    let types: Vec<_> = lines.iter().filter_map(|v| v["type"].as_str()).collect();
    assert!(types.contains(&"TOOL_CALL"), "TOOL_CALL must be present");
    assert!(
        types.contains(&"POLICY_DECISION"),
        "POLICY_DECISION must be present"
    );
}

/// Zero event loss under burst of 200 events.
#[test]
fn audit_logger_no_event_loss_under_burst() {
    let dir = tempdir().unwrap();
    let sink = RollingFileSink::new(dir.path(), "burst", 0).unwrap();
    let logger = AuditLogger::with_sinks(vec![Box::new(sink)]);

    const N: u32 = 200;
    for i in 0..N {
        logger.log(tool_call(i));
    }

    let lines = wait_for_sink_events(dir.path(), N as usize, Duration::from_secs(5));
    assert_eq!(
        lines.len(),
        N as usize,
        "all {N} events must be persisted; got {}",
        lines.len()
    );
}

/// File rotation during burst produces valid NDJSON across all files.
#[test]
fn audit_logger_rotated_files_all_contain_valid_ndjson() {
    let dir = tempdir().unwrap();
    // Very small rotation limit forces multiple files.
    let sink = RollingFileSink::new(dir.path(), "rot", 64).unwrap();
    let logger = AuditLogger::with_sinks(vec![Box::new(sink)]);

    const N: u32 = 50;
    for i in 0..N {
        logger.log(tool_call(i));
    }

    let lines = wait_for_sink_events(dir.path(), N as usize, Duration::from_secs(5));
    let file_count = fs::read_dir(dir.path()).unwrap().count();
    assert!(
        file_count >= 2,
        "rotation must have occurred; got {file_count} file(s)"
    );
    assert_eq!(lines.len(), N as usize, "no events lost during rotation");
}

/// `MeteredSink` reports correct event count via counters (used for retention metrics).
#[test]
fn metered_sink_retention_metrics_are_accurate() {
    let dir = tempdir().unwrap();
    let inner = RollingFileSink::new(dir.path(), "metered", 0).unwrap();
    // Wrap in MeteredSink (simulates MCP audit_retention_snapshot metrics).
    let metered = std::sync::Arc::new(MeteredSink::new(inner));
    let metered_clone = std::sync::Arc::clone(&metered);

    let logger = AuditLogger::with_sinks(vec![Box::new({
        // Adapter: MeteredSink<RollingFileSink> needs to be boxed as dyn AuditSink.
        // We use a wrapper since Arc<MeteredSink> doesn't impl AuditSink directly.
        MeteredSinkAdapter {
            inner: metered_clone,
        }
    })]);

    const N: u32 = 20;
    for i in 0..N {
        logger.log(tool_call(i));
    }

    // Wait for all events to be processed by the worker thread.
    let _ = wait_for_sink_events(dir.path(), N as usize, Duration::from_secs(3));
    assert_eq!(
        metered.events_written(),
        N as u64,
        "MeteredSink must count every persisted event"
    );
    assert_eq!(metered.errors(), 0, "no errors expected");
}

/// `AuditLogger::new()` (no sinks) behaves exactly as before — no regression.
#[test]
fn audit_logger_without_sinks_still_buffers_in_memory() {
    let logger = AuditLogger::new();
    logger.log(tool_call(1));
    logger.log(policy_event(2));

    // In-memory buffer is populated synchronously.
    let events = logger.recent_events();
    assert_eq!(
        events.len(),
        2,
        "in-memory buffer must still work without sinks"
    );
}

/// PII is redacted before events reach the persistent sink.
#[test]
fn audit_logger_sink_receives_redacted_events() {
    let dir = tempdir().unwrap();
    let sink = RollingFileSink::new(dir.path(), "pii", 0).unwrap();
    let logger = AuditLogger::with_sinks(vec![Box::new(sink)]);

    logger.log(AuditEvent::ToolCall {
        tool_name: "fill".into(),
        args: json!({ "value": "alice@example.com", "selector": "#email" }),
        timestamp: 0,
    });

    let lines = wait_for_sink_events(dir.path(), 1, Duration::from_secs(2));
    assert_eq!(lines.len(), 1);
    let args = &lines[0]["args"];
    assert_eq!(
        args["value"].as_str().unwrap_or(""),
        "***",
        "email must be redacted before reaching the sink"
    );
    assert_eq!(args["selector"].as_str().unwrap_or(""), "#email");
}

// ---------------------------------------------------------------------------
// Helper: Arc<MeteredSink> adapter for dyn AuditSink
// ---------------------------------------------------------------------------

struct MeteredSinkAdapter {
    inner: std::sync::Arc<MeteredSink<RollingFileSink>>,
}

impl AuditSink for MeteredSinkAdapter {
    fn write(&self, event: &AuditEvent) -> Result<(), core_runtime::audit_sink::AuditSinkError> {
        self.inner.write(event)
    }

    fn name(&self) -> &str {
        "MeteredSinkAdapter"
    }
}
