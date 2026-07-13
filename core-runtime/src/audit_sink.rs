/// Persistent audit sink implementations for `AuditLogger`.
///
/// Two mandatory sinks (PR-24 / ISSUE-09):
///
/// * `RollingFileSink` — writes newline-delimited JSON (NDJSON) audit events
///   to rotating files; rotates when a file reaches `max_bytes_per_file`.
///
/// * `WebhookSink` — POSTs each event as JSON to an HTTPS endpoint
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
    #[error("Invalid sink URL '{0}': HTTPS is required except for literal loopback HTTP")]
    InvalidUrl(String),
    #[error("Failed to configure webhook HTTP client: {0}")]
    HttpClient(String),
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

/// Controls how written data is flushed to storage after each event.
///
/// Choose `Sync` for high-durability audit deployments where data must
/// survive an abrupt process or power failure.  Choose `Flush` (the default)
/// for throughput-sensitive deployments where OS-level buffering is acceptable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DurabilityMode {
    /// Flush user-space buffers to the OS after each write (`flush()`).
    /// Fast — data may still reside in the OS page cache.
    #[default]
    Flush,
    /// Flush user-space buffers **and** issue `sync_data()` after each write.
    /// Slower but guarantees the data reaches durable storage before returning,
    /// satisfying enterprise audit-retention requirements (ISSUE-14).
    Sync,
}

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
///
/// Durability is configurable via [`DurabilityMode`]:
/// - `Flush` (default) — fast, OS-level buffering.
/// - `Sync` — issues `sync_data()` after each write for crash-safe retention.
pub struct RollingFileSink {
    dir: PathBuf,
    prefix: String,
    max_bytes_per_file: u64,
    durability: DurabilityMode,
    state: Mutex<Option<RollingFileState>>,
}

impl RollingFileSink {
    /// Create a new sink with [`DurabilityMode::Flush`] (default).
    ///
    /// The directory is created if it does not exist.
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
            durability: DurabilityMode::Flush,
            state: Mutex::new(None),
        })
    }

    /// Set the durability mode (builder style).
    ///
    /// Use [`DurabilityMode::Sync`] for enterprise audit-retention compliance.
    pub fn with_durability(mut self, mode: DurabilityMode) -> Self {
        self.durability = mode;
        self
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
        match self.durability {
            DurabilityMode::Flush => state.current_file.flush()?,
            DurabilityMode::Sync => {
                state.current_file.flush()?;
                state.current_file.sync_data()?;
            }
        }
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

/// POSTs audit events as JSON to an HTTPS endpoint for SIEM integration.
///
/// `write()` enqueues the serialized event into a bounded internal channel and
/// returns immediately, so it never blocks the audit worker thread.  A
/// dedicated background thread drains the queue and performs the actual HTTP
/// POST with linear back-off retries.  If the queue is full (SIEM unreachable
/// for an extended period) excess events are dropped with an `eprintln!`
/// warning — SIEM delivery is best-effort.
///
/// Plaintext HTTP is accepted only for literal loopback IP addresses, which
/// supports local development without exposing audit data on the network.
#[derive(Debug)]
pub struct WebhookSink {
    sender: crossbeam_channel::Sender<String>,
}

impl WebhookSink {
    /// Create a new webhook sink and spawn its background sender thread.
    ///
    /// * `url` — HTTPS endpoint, or a literal loopback HTTP endpoint.
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
        let url = validate_webhook_url(url.into())?;
        let client = build_webhook_client(url.scheme() == "http")?;

        let (sender, receiver) = bounded::<String>(queue_capacity.max(1));
        let url_for_thread = url.to_string();

        thread::spawn(move || {
            while let Ok(body) = receiver.recv() {
                if let Err((attempts, last_error)) =
                    retry_webhook_post(max_retries, retry_delay, || {
                        post_once(&client, &url_for_thread, &body)
                    })
                {
                    eprintln!(
                        "[AUDIT][ERROR] WebhookSink failed after {} attempt(s): {}",
                        attempts, last_error
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

fn validate_webhook_url(url: String) -> Result<url::Url, AuditSinkError> {
    use url::Host;

    let parsed = url::Url::parse(&url).map_err(|_| AuditSinkError::InvalidUrl(url.clone()))?;
    if !parsed.username().is_empty() || parsed.password().is_some() || parsed.fragment().is_some() {
        return Err(AuditSinkError::InvalidUrl(url));
    }

    let allowed = match (parsed.scheme(), parsed.host()) {
        ("https", Some(_)) => true,
        ("http", Some(Host::Ipv4(address))) => address.is_loopback(),
        ("http", Some(Host::Ipv6(address))) => address.is_loopback(),
        _ => false,
    };
    if !allowed {
        return Err(AuditSinkError::InvalidUrl(url));
    }
    Ok(parsed)
}

fn build_webhook_client(disable_proxy: bool) -> Result<reqwest::blocking::Client, AuditSinkError> {
    let mut builder = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(5));
    if disable_proxy {
        builder = builder.no_proxy();
    }
    builder
        .build()
        .map_err(|error| AuditSinkError::HttpClient(error.without_url().to_string()))
}

#[derive(Debug)]
struct WebhookPostError {
    message: String,
    retryable: bool,
}

fn retry_webhook_post(
    max_retries: u32,
    retry_delay: Duration,
    mut post: impl FnMut() -> Result<(), WebhookPostError>,
) -> Result<u32, (u32, String)> {
    for attempt in 0..=max_retries {
        match post() {
            Ok(()) => return Ok(attempt + 1),
            Err(error) if attempt < max_retries && error.retryable => {
                thread::sleep(retry_delay);
            }
            Err(error) => return Err((attempt + 1, error.message)),
        }
    }
    unreachable!("the inclusive retry loop always executes at least once")
}

fn post_once(
    client: &reqwest::blocking::Client,
    url: &str,
    body: &str,
) -> Result<(), WebhookPostError> {
    let response = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body.to_owned())
        .send()
        .map_err(|error| WebhookPostError {
            message: error.without_url().to_string(),
            retryable: true,
        })?;

    let status = response.status();
    if status.is_success() {
        return Ok(());
    }

    Err(WebhookPostError {
        message: format!("HTTP {status}"),
        retryable: matches!(status.as_u16(), 408 | 425 | 429) || status.is_server_error(),
    })
}

// ---------------------------------------------------------------------------
// SinkMetrics (observable statistics)
// ---------------------------------------------------------------------------

/// Wraps any `AuditSink` and counts events written, bytes written, and errors.
pub struct MeteredSink<S: AuditSink> {
    inner: S,
    events_written: std::sync::atomic::AtomicU64,
    bytes_written: std::sync::atomic::AtomicU64,
    errors: std::sync::atomic::AtomicU64,
}

impl<S: AuditSink> MeteredSink<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            events_written: std::sync::atomic::AtomicU64::new(0),
            bytes_written: std::sync::atomic::AtomicU64::new(0),
            errors: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn events_written(&self) -> u64 {
        self.events_written
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Total bytes of serialized events successfully written.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
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
                if let Ok(serialized) = serde_json::to_string(event) {
                    // +1 for the trailing '\n' that RollingFileSink appends to every line.
                    self.bytes_written.fetch_add(
                        serialized.len() as u64 + 1,
                        std::sync::atomic::Ordering::Relaxed,
                    );
                }
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

impl<S: AuditSink> AuditSink for std::sync::Arc<S> {
    fn write(&self, event: &AuditEvent) -> Result<(), AuditSinkError> {
        (**self).write(event)
    }

    fn name(&self) -> &str {
        (**self).name()
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{generate_simple_self_signed, CertifiedKey};
    use rustls::{ServerConfig, ServerConnection, StreamOwned};
    use serde_json::json;
    use std::{
        fs,
        io::{Read, Write},
        net::TcpListener,
        sync::{mpsc, Arc},
    };
    use tempfile::tempdir;

    fn tool_call_event(n: u32) -> AuditEvent {
        AuditEvent::ToolCall {
            tool_name: format!("tool_{n}"),
            args: json!({ "n": n }),
            timestamp: n as u64,
        }
    }

    fn spawn_tls_server() -> (
        u16,
        reqwest::Certificate,
        mpsc::Receiver<String>,
        thread::JoinHandle<()>,
    ) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let CertifiedKey { cert, key_pair } =
            generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let trusted_cert = reqwest::Certificate::from_der(cert.der().as_ref()).unwrap();
        let private_key = rustls::pki_types::PrivateKeyDer::Pkcs8(
            rustls::pki_types::PrivatePkcs8KeyDer::from(key_pair.serialize_der()),
        );
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert.der().clone()], private_key)
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (request_tx, request_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let connection = ServerConnection::new(Arc::new(config)).unwrap();
            let mut stream = StreamOwned::new(connection, stream);
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let Ok(read) = stream.read(&mut buffer) else {
                    return;
                };
                if read == 0 {
                    return;
                }
                request.extend_from_slice(&buffer[..read]);
                if let Some(header_end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            line.split_once(':').and_then(|(name, value)| {
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())
                                    .flatten()
                            })
                        })
                        .unwrap_or(0);
                    if request.len() >= header_end + 4 + content_length {
                        break;
                    }
                }
            }
            let _ = request_tx.send(String::from_utf8(request).unwrap());
            let _ = stream.write_all(
                b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            let _ = stream.flush();
        });
        (port, trusted_cert, request_rx, handle)
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

        let content = fs::read_to_string(files[0].path()).unwrap();
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

    // -- DurabilityMode / RollingFileSink high-durability mode ----------------

    #[test]
    fn rolling_file_sink_sync_mode_persists_events() {
        let dir = tempdir().unwrap();
        let sink = RollingFileSink::new(dir.path(), "sync", 0)
            .unwrap()
            .with_durability(DurabilityMode::Sync);
        for i in 0..20u32 {
            sink.write(&tool_call_event(i)).unwrap();
        }
        let total: usize = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| {
                fs::read_to_string(e.path())
                    .unwrap_or_default()
                    .lines()
                    .count()
            })
            .sum();
        assert_eq!(total, 20, "sync mode must persist all events");
    }

    #[test]
    fn rolling_file_sink_sync_mode_data_readable_after_sink_dropped() {
        let dir = tempdir().unwrap();
        {
            let sink = RollingFileSink::new(dir.path(), "drop", 0)
                .unwrap()
                .with_durability(DurabilityMode::Sync);
            sink.write(&tool_call_event(1)).unwrap();
            sink.write(&tool_call_event(2)).unwrap();
            // sink dropped here — data must be fully on disk
        }
        let total: usize = fs::read_dir(dir.path())
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
            total, 2,
            "sync mode must durably persist events before drop"
        );
    }

    #[test]
    fn rolling_file_sink_default_mode_is_flush() {
        let dir = tempdir().unwrap();
        let sink = RollingFileSink::new(dir.path(), "default", 0).unwrap();
        // Verify the default mode doesn't panic — flush mode works correctly.
        sink.write(&tool_call_event(1)).unwrap();
        let total: usize = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| {
                fs::read_to_string(e.path())
                    .unwrap_or_default()
                    .lines()
                    .count()
            })
            .sum();
        assert_eq!(total, 1);
    }

    // -- WebhookSink ----------------------------------------------------------

    #[test]
    fn webhook_sink_accepts_https_url_at_construction() {
        let sink = WebhookSink::new("https://siem.example.com/events", 1, Duration::ZERO, 8);
        assert!(sink.is_ok(), "https:// must be accepted: {sink:?}");
    }

    #[test]
    fn webhook_sink_rejects_non_loopback_plain_http() {
        for url in [
            "http://192.0.2.1/events",
            "http://example.com/events",
            "http://[2001:db8::1]/events",
            "http://localhost/events",
        ] {
            let err = WebhookSink::new(url, 0, Duration::ZERO, 8).unwrap_err();
            assert!(
                matches!(err, AuditSinkError::InvalidUrl(_)),
                "non-loopback plaintext URL must be rejected: {url}"
            );
        }
    }

    #[test]
    fn webhook_sink_accepts_literal_loopback_plain_http() {
        for url in ["http://127.0.0.1:1/events", "http://[::1]:1/events"] {
            let sink = WebhookSink::new(url, 0, Duration::ZERO, 8);
            assert!(sink.is_ok(), "literal loopback URL must be accepted: {url}");
        }
    }

    #[test]
    fn webhook_sink_rejects_unsafe_url_components_and_schemes() {
        for url in [
            "https://user:secret@siem.example.com/events",
            "https://siem.example.com/events#fragment",
            "ftp://siem.example.com/events",
            "https://",
        ] {
            let err = WebhookSink::new(url, 0, Duration::ZERO, 8).unwrap_err();
            assert!(
                matches!(err, AuditSinkError::InvalidUrl(_)),
                "unsafe URL must be rejected: {url}"
            );
        }
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

    #[test]
    fn webhook_https_posts_json_with_a_trusted_certificate() {
        let (port, trusted_cert, request_rx, handle) = spawn_tls_server();
        let client = reqwest::blocking::Client::builder()
            .add_root_certificate(trusted_cert)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let body = serde_json::to_string(&tool_call_event(7)).unwrap();

        post_once(&client, &format!("https://localhost:{port}/events"), &body).unwrap();

        let request = request_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(request.starts_with("POST /events HTTP/1.1\r\n"));
        assert!(request
            .to_ascii_lowercase()
            .contains("content-type: application/json"));
        assert!(request.ends_with(&body));
        handle.join().unwrap();
    }

    #[test]
    fn webhook_https_rejects_untrusted_certificate() {
        let (port, _trusted_cert, _request_rx, handle) = spawn_tls_server();
        let client = build_webhook_client(true).unwrap();

        let error =
            post_once(&client, &format!("https://localhost:{port}/events"), "{}").unwrap_err();

        assert!(error.retryable);
        assert!(!error.message.contains("localhost"));
        handle.join().unwrap();
    }

    #[test]
    fn webhook_https_rejects_hostname_mismatch() {
        let (port, trusted_cert, _request_rx, handle) = spawn_tls_server();
        let client = reqwest::blocking::Client::builder()
            .add_root_certificate(trusted_cert)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();

        let error =
            post_once(&client, &format!("https://127.0.0.1:{port}/events"), "{}").unwrap_err();

        assert!(error.retryable);
        assert!(!error.message.contains("127.0.0.1"));
        handle.join().unwrap();
    }

    #[test]
    fn webhook_post_does_not_follow_redirects() {
        let redirect_target = TcpListener::bind("127.0.0.1:0").unwrap();
        redirect_target.set_nonblocking(true).unwrap();
        let target_port = redirect_target.local_addr().unwrap().port();
        let redirect_server = TcpListener::bind("127.0.0.1:0").unwrap();
        let redirect_port = redirect_server.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            let (mut stream, _) = redirect_server.accept().unwrap();
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer).unwrap();
            write!(
                stream,
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:{target_port}/leak\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
        });
        let client = build_webhook_client(true).unwrap();

        let error = post_once(
            &client,
            &format!("http://127.0.0.1:{redirect_port}/events"),
            "{\"secret\":true}",
        )
        .unwrap_err();

        assert_eq!(error.message, "HTTP 307 Temporary Redirect");
        assert!(!error.retryable);
        handle.join().unwrap();
        assert!(
            matches!(redirect_target.accept(), Err(error) if error.kind() == io::ErrorKind::WouldBlock),
            "redirect target must not receive the audit body"
        );
    }

    #[test]
    fn webhook_non_retryable_status_reports_one_attempt() {
        let mut calls = 0;

        let error = retry_webhook_post(3, Duration::ZERO, || {
            calls += 1;
            Err(WebhookPostError {
                message: "HTTP 400 Bad Request".to_string(),
                retryable: false,
            })
        })
        .unwrap_err();

        assert_eq!(calls, 1);
        assert_eq!(error, (1, "HTTP 400 Bad Request".to_string()));
    }
}
