/// Persistent audit sink implementations for `AuditLogger`.
///
/// Two mandatory sinks (PR-24 / ISSUE-09):
///
/// * `RollingFileSink` — writes newline-delimited JSON (NDJSON) audit events
///   to rotating files; rotates when a file reaches `max_bytes_per_file`.
///
/// * `WebhookSink` — POSTs each event as JSON to an HTTP (`http://`) endpoint
///   for SIEM integration; retries transiently failing requests with linear
///   back-off on a background thread (non-blocking to the audit worker).
use crate::audit::AuditEvent;
use crossbeam_channel::{bounded, TrySendError};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write as IoWrite},
    path::{Path, PathBuf},
    sync::Mutex,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum AuditSinkError {
    #[error("I/O error writing audit sink: {0}")]
    Io(#[from] io::Error),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Webhook POST failed after {attempts} attempt(s): {last_error}")]
    WebhookFailed { attempts: u32, last_error: String },
    #[error("Invalid sink URL '{0}': only http:// is supported")]
    InvalidUrl(String),
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// A sink that receives sanitized `AuditEvent`s for persistent storage.
pub trait AuditSink: Send + Sync {
    /// Write one event.  Implementations must be non-blocking or use an
    /// internal background thread; they must NOT block the audit worker for
    /// more than a brief moment.
    fn write(&self, event: &AuditEvent) -> Result<(), AuditSinkError>;

    /// Human-readable name used in log messages.
    fn name(&self) -> &str;
}

// ---------------------------------------------------------------------------
// RollingFileSink
// ---------------------------------------------------------------------------

struct RollingFileState {
    current_file: File,
    #[allow(dead_code)]
    current_path: PathBuf,
    current_bytes: u64,
    #[allow(dead_code)]
    sequence: u64,
}

/// Writes NDJSON audit events to rolling files inside `dir`.
///
/// File names follow the pattern `{prefix}_{unix_ms}_{seq}.ndjson`.
/// When the current file reaches `max_bytes_per_file` the sink opens a new
/// file on the next `write` call.
pub struct RollingFileSink {
    dir: PathBuf,
    prefix: String,
    max_bytes_per_file: u64,
    state: Mutex<Option<RollingFileState>>,
}

impl RollingFileSink {
    /// Create a new sink.  The directory is created if it does not exist.
    ///
    /// * `dir` — directory that will hold the log files.
    /// * `prefix` — file name prefix (e.g. `"audit"`).
    /// * `max_bytes_per_file` — rotate after this many bytes (0 = never rotate).
    pub fn new(
        dir: impl AsRef<Path>,
        prefix: impl Into<String>,
        max_bytes_per_file: u64,
    ) -> Result<Self, AuditSinkError> {
        let dir = dir.as_ref().to_path_buf();
        fs::create_dir_all(&dir)?;
        Ok(Self {
            dir,
            prefix: prefix.into(),
            max_bytes_per_file,
            state: Mutex::new(None),
        })
    }

    fn open_new_file(&self) -> Result<RollingFileState, AuditSinkError> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();

        // Use a global sequence counter derived from timestamp to keep names unique.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let filename = format!("{}_{}_{}.ndjson", self.prefix, ts, seq);
        let path = self.dir.join(&filename);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(RollingFileState {
            current_file: file,
            current_path: path,
            current_bytes: 0,
            sequence: seq,
        })
    }
}

impl AuditSink for RollingFileSink {
    fn write(&self, event: &AuditEvent) -> Result<(), AuditSinkError> {
        let line = {
            let mut s = serde_json::to_string(event)?;
            s.push('\n');
            s
        };
        let bytes = line.len() as u64;

        let mut guard = self.state.lock().expect("audit sink mutex poisoned");

        // Initialise on first write or rotate if over limit.
        let needs_new = match guard.as_ref() {
            None => true,
            Some(s) => {
                self.max_bytes_per_file > 0 && s.current_bytes + bytes > self.max_bytes_per_file
            }
        };

        if needs_new {
            *guard = Some(self.open_new_file()?);
        }

        let state = guard.as_mut().expect("state must be Some after init");
        state.current_file.write_all(line.as_bytes())?;
        state.current_file.flush()?;
        state.current_bytes += bytes;
        Ok(())
    }

    fn name(&self) -> &str {
        "RollingFileSink"
    }
}

// ---------------------------------------------------------------------------
// WebhookSink
// ---------------------------------------------------------------------------

/// POSTs audit events as JSON to an HTTP (plaintext) endpoint for SIEM
/// integration.
///
/// `write()` enqueues the serialized event into a bounded internal channel and
/// returns immediately, so it never blocks the audit worker thread.  A
/// dedicated background thread drains the queue and performs the actual HTTP
/// POST with linear back-off retries.  If the queue is full (SIEM unreachable
/// for an extended period) excess events are dropped with an `eprintln!`
/// warning — SIEM delivery is best-effort.
///
/// **Note**: only `http://` URLs are supported.  Passing an `https://` URL
/// returns an [`AuditSinkError::InvalidUrl`] at construction time.
#[derive(Debug)]
pub struct WebhookSink {
    sender: crossbeam_channel::Sender<String>,
}

impl WebhookSink {
    /// Create a new webhook sink and spawn its background sender thread.
    ///
    /// * `url` — target HTTP endpoint (`http://` scheme only).
    /// * `max_retries` — retry attempts after the first failure (0 = no retry).
    /// * `retry_delay` — pause between retries.
    /// * `queue_capacity` — max events queued while SIEM is unavailable; older
    ///   events are dropped when the queue is full.
    pub fn new(
        url: impl Into<String>,
        max_retries: u32,
        retry_delay: Duration,
        queue_capacity: usize,
    ) -> Result<Self, AuditSinkError> {
        let url = url.into();
        if !url.starts_with("http://") {
            return Err(AuditSinkError::InvalidUrl(url));
        }

        let (sender, receiver) = bounded::<String>(queue_capacity.max(1));
        let url_for_thread = url;

        thread::spawn(move || {
            while let Ok(body) = receiver.recv() {
                let mut last_error = String::new();
                for attempt in 0..=max_retries {
                    match post_once(&url_for_thread, &body) {
                        Ok(()) => {
                            last_error.clear();
                            break;
                        }
                        Err(e) => {
                            last_error = e;
                            if attempt < max_retries {
                                thread::sleep(retry_delay);
                            }
                        }
                    }
                }
                if !last_error.is_empty() {
                    eprintln!(
                        "[AUDIT][ERROR] WebhookSink failed after {} attempt(s): {}",
                        max_retries + 1,
                        last_error
                    );
                }
            }
        });

        Ok(Self { sender })
    }
}

impl AuditSink for WebhookSink {
    /// Enqueues the event for delivery by the background thread.
    ///
    /// Always returns `Ok(())` — delivery errors are logged via `eprintln!`
    /// on the background thread.  If the internal queue is full, the event is
    /// dropped (SIEM is best-effort).
    fn write(&self, event: &AuditEvent) -> Result<(), AuditSinkError> {
        let body = serde_json::to_string(event)?;
        match self.sender.try_send(body) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                eprintln!("[AUDIT][WARN] WebhookSink queue full — event dropped");
            }
            Err(TrySendError::Disconnected(_)) => {
                eprintln!("[AUDIT][ERROR] WebhookSink background thread exited unexpectedly");
            }
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "WebhookSink"
    }
}

fn post_once(url: &str, body: &str) -> Result<(), String> {
    use std::io::Read;
    use std::net::TcpStream;

    let without_scheme = url.trim_start_matches("http://");
    let (host_port, path) = match without_scheme.find('/') {
        Some(idx) => (&without_scheme[..idx], &without_scheme[idx..]),
        None => (without_scheme, "/"),
    };

    let mut stream =
        TcpStream::connect(host_port).map_err(|e| format!("connect to {host_port}: {e}"))?;
    stream.set_write_timeout(Some(Duration::from_secs(5))).ok();
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();

    let request = format!(
        "POST {path} HTTP/1.0\r\n\
         Host: {host_port}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        len = body.len()
    );

    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("write request: {e}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| format!("read response: {e}"))?;

    let status_line = response.lines().next().unwrap_or("");
    let status: u32 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(format!("HTTP {status}: {status_line}"))
    }
}

// ---------------------------------------------------------------------------
// SinkMetrics (observable statistics)
// ---------------------------------------------------------------------------

/// Wraps any `AuditSink` and counts events written and errors.
pub struct MeteredSink<S: AuditSink> {
    inner: S,
    events_written: std::sync::atomic::AtomicU64,
    errors: std::sync::atomic::AtomicU64,
}

impl<S: AuditSink> MeteredSink<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            events_written: std::sync::atomic::AtomicU64::new(0),
            errors: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn events_written(&self) -> u64 {
        self.events_written
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn errors(&self) -> u64 {
        self.errors.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl<S: AuditSink> AuditSink for MeteredSink<S> {
    fn write(&self, event: &AuditEvent) -> Result<(), AuditSinkError> {
        match self.inner.write(event) {
            Ok(()) => {
                self.events_written
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Ok(())
            }
            Err(e) => {
                self.errors
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Err(e)
            }
        }
    }

    fn name(&self) -> &str {
        self.inner.name()
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::tempdir;

    fn tool_call_event(n: u32) -> AuditEvent {
        AuditEvent::ToolCall {
            tool_name: format!("tool_{n}"),
            args: json!({ "n": n }),
            timestamp: n as u64,
        }
    }

    // -- RollingFileSink ------------------------------------------------------

    #[test]
    fn rolling_file_sink_creates_directory_and_writes_ndjson() {
        let dir = tempdir().unwrap();
        let sink = RollingFileSink::new(dir.path(), "audit", 0).unwrap();
        sink.write(&tool_call_event(1)).unwrap();
        sink.write(&tool_call_event(2)).unwrap();

        let files: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(files.len(), 1, "no rotation expected when max_bytes=0");

        let content = fs::read_to_string(&files[0].path()).unwrap();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("TOOL_CALL"));
        assert!(lines[1].contains("TOOL_CALL"));
    }

    #[test]
    fn rolling_file_sink_rotates_when_file_exceeds_limit() {
        let dir = tempdir().unwrap();
        // Very small limit forces rotation after first event.
        let sink = RollingFileSink::new(dir.path(), "audit", 1).unwrap();
        for i in 0..5 {
            sink.write(&tool_call_event(i)).unwrap();
        }

        let file_count = fs::read_dir(dir.path()).unwrap().count();
        assert!(
            file_count >= 2,
            "should have rotated at least once; got {file_count} files"
        );
    }

    #[test]
    fn rolling_file_sink_all_events_persisted_after_burst() {
        let dir = tempdir().unwrap();
        let sink = RollingFileSink::new(dir.path(), "burst", 512).unwrap();

        let n = 200u32;
        for i in 0..n {
            sink.write(&tool_call_event(i)).unwrap();
        }

        // Count lines across all rotated files.
        let total_lines: usize = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| {
                fs::read_to_string(e.path())
                    .unwrap_or_default()
                    .lines()
                    .count()
            })
            .sum();

        assert_eq!(
            total_lines, n as usize,
            "every event must be persisted; got {total_lines}"
        );
    }

    #[test]
    fn rolling_file_sink_written_lines_are_valid_json() {
        let dir = tempdir().unwrap();
        let sink = RollingFileSink::new(dir.path(), "json", 0).unwrap();
        let events = vec![
            AuditEvent::ToolCall {
                tool_name: "act".into(),
                args: json!({ "selector": "#btn" }),
                timestamp: 42,
            },
            AuditEvent::PolicyDecision {
                rule_id: "R1".into(),
                action: "click".into(),
                decision: "allow".into(),
                timestamp: 43,
            },
        ];
        for e in &events {
            sink.write(e).unwrap();
        }

        let content = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| fs::read_to_string(e.path()).unwrap_or_default())
            .collect::<String>();

        for line in content.lines() {
            serde_json::from_str::<serde_json::Value>(line).expect("every line must be valid JSON");
        }
    }

    // -- MeteredSink ----------------------------------------------------------

    #[test]
    fn metered_sink_counts_events() {
        let dir = tempdir().unwrap();
        let inner = RollingFileSink::new(dir.path(), "metered", 0).unwrap();
        let sink = MeteredSink::new(inner);

        for i in 0..10 {
            sink.write(&tool_call_event(i)).unwrap();
        }
        assert_eq!(sink.events_written(), 10);
        assert_eq!(sink.errors(), 0);
    }

    // -- WebhookSink ----------------------------------------------------------

    #[test]
    fn webhook_sink_rejects_https_url_at_construction() {
        let err =
            WebhookSink::new("https://siem.example.com/events", 1, Duration::ZERO, 8).unwrap_err();
        assert!(
            matches!(err, AuditSinkError::InvalidUrl(_)),
            "https:// must be rejected; got: {err}"
        );
    }

    #[test]
    fn webhook_sink_write_returns_ok_immediately_even_when_server_unreachable() {
        // Port 1 is reserved/refused on all platforms — no server will answer.
        let sink = WebhookSink::new("http://127.0.0.1:1", 0, Duration::ZERO, 8).unwrap();
        // write() must return Ok(()) without blocking (background thread handles retries).
        let result = sink.write(&tool_call_event(1));
        assert!(result.is_ok(), "write() must be non-blocking and return Ok");
    }

    #[test]
    fn webhook_sink_rejects_bare_url_without_scheme() {
        let err = WebhookSink::new("siem.example.com/events", 0, Duration::ZERO, 8).unwrap_err();
        assert!(matches!(err, AuditSinkError::InvalidUrl(_)));
    }
}
