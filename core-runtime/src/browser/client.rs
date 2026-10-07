use super::*;

impl BrowserClient {
    pub fn new() -> Result<Self> {
        use rand::RngCore;
        let mut key = [0u8; 32];
        rand::rng().fill_bytes(&mut key);
        let kms = Box::new(SoftwareKms::new(key, "default-key".to_string()));
        let vault = Arc::new(LocalSessionVault::new(kms));
        // Every BrowserClient needs *some* vault to satisfy PageSession's required field
        // (ISSUE-210) — go through the private helper, not the public, feature-gated
        // `new_with_vault`, so this constructor keeps working with `session-vault-api` off.
        Self::new_with_vault_and_path(vault, None)
    }

    /// Create a `BrowserClient` with plugin hook integration.
    ///
    /// The provided `PluginHookConfig` is shared across all `PageSession`
    /// instances created from this client.  Use `PluginHookConfig::default()`
    /// (or the `new()` constructor) to get backward-compatible behaviour with
    /// no active plugins.
    pub fn new_with_plugin_hooks(plugin_hooks: PluginHookConfig) -> Result<Self> {
        use rand::RngCore;
        let mut key = [0u8; 32];
        rand::rng().fill_bytes(&mut key);
        let kms = Box::new(SoftwareKms::new(key, "default-key".to_string()));
        let vault = Arc::new(LocalSessionVault::new(kms));
        Self::new_with_vault_and_path_and_hooks(vault, None, plugin_hooks)
    }

    /// Create a `BrowserClient` with both an explicit Chrome path and plugin hook integration
    /// (ISSUE-303). `new_with_chrome_path` and `new_with_plugin_hooks` each cover one of these
    /// two axes; production startup (`dragon-head-mcp`) needs both at once.
    pub fn new_with_chrome_path_and_plugin_hooks(
        chrome_path: Option<String>,
        plugin_hooks: PluginHookConfig,
    ) -> Result<Self> {
        use rand::RngCore;
        let mut key = [0u8; 32];
        rand::rng().fill_bytes(&mut key);
        let kms = Box::new(SoftwareKms::new(key, "default-key".to_string()));
        let vault = Arc::new(LocalSessionVault::new(kms));
        Self::new_with_vault_and_path_and_hooks(vault, chrome_path, plugin_hooks)
    }

    pub fn new_with_chrome_path(chrome_path: Option<String>) -> Result<Self> {
        use rand::RngCore;
        let mut key = [0u8; 32];
        rand::rng().fill_bytes(&mut key);
        let kms = Box::new(SoftwareKms::new(key, "default-key".to_string()));
        let vault = Arc::new(LocalSessionVault::new(kms));
        Self::new_with_vault_and_path(vault, chrome_path)
    }

    /// Create a `BrowserClient` with an explicit window size.
    ///
    /// Used in tests to verify that different viewport dimensions produce
    /// different stable_key quadrant assignments for the same element.
    pub fn new_with_window_size(width: u32, height: u32) -> Result<Self> {
        use rand::RngCore;
        let mut key = [0u8; 32];
        rand::rng().fill_bytes(&mut key);
        let kms = Box::new(SoftwareKms::new(key, "default-key".to_string()));
        let vault = Arc::new(LocalSessionVault::new(kms));
        Self::new_with_vault_and_size(vault, width, height)
    }

    /// Create a `BrowserClient` backed by a caller-supplied `SessionVault` (e.g. a durable,
    /// KMS-backed implementation instead of the in-memory default — see
    /// `docs/operations.md`'s "Session Vault key management procedure").
    ///
    /// Gated behind the `session-vault-api` feature (ISSUE-210): without
    /// `save_to_vault`/`load_from_vault` (also gated) actually reading/writing through it, a
    /// custom vault injected here has no observable effect, so this constructor is exactly the
    /// kind of unreachable-without-a-caller surface the issue asks to gate. See
    /// `docs/session-vault.md`.
    #[cfg(feature = "session-vault-api")]
    pub fn new_with_vault(vault: Arc<dyn SessionVault>) -> Result<Self> {
        Self::new_with_vault_and_path(vault, None)
    }

    pub(super) fn new_with_vault_and_path(
        vault: Arc<dyn SessionVault>,
        chrome_path: Option<String>,
    ) -> Result<Self> {
        Self::new_with_vault_and_path_and_hooks(vault, chrome_path, PluginHookConfig::default())
    }

    pub(super) fn new_with_vault_and_path_and_hooks(
        vault: Arc<dyn SessionVault>,
        chrome_path: Option<String>,
        plugin_hooks: PluginHookConfig,
    ) -> Result<Self> {
        let mut builder = LaunchOptions::default_builder();
        builder.headless(true);
        if let Some(path) = chrome_path.as_ref() {
            builder.path(Some(path.clone().into()));
        }
        let options = builder.build().context("Failed to build launch options")?;

        let browser = Browser::new(options).context("Failed to launch browser")?;
        Ok(Self {
            inner: browser,
            vault,
            plugin_hooks: Arc::new(plugin_hooks),
            viewport_size: None,
            chrome_path,
        })
    }

    pub(super) fn new_with_vault_and_size(
        vault: Arc<dyn SessionVault>,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        let mut builder = LaunchOptions::default_builder();
        builder.headless(true).window_size(Some((width, height)));
        let options = builder.build().context("Failed to build launch options")?;

        let browser = Browser::new(options).context("Failed to launch browser")?;
        Ok(Self {
            inner: browser,
            vault,
            plugin_hooks: Arc::new(PluginHookConfig::default()),
            viewport_size: Some((width, height)),
            chrome_path: None,
        })
    }

    pub fn new_page(&self) -> Result<PageSession> {
        self.new_page_with_audit_logger(AuditLogger::from_env())
    }

    /// Like [`new_page`](Self::new_page), but uses the given [`AuditLogger`] instead of one
    /// built from `std::env` via [`AuditLogger::from_env`]. Lets callers (e.g. the MCP server
    /// binary) merge config-file settings with env vars via [`AuditLogger::from_env_with`]
    /// without mutating global environment state.
    pub fn new_page_with_audit_logger(&self, audit_logger: AuditLogger) -> Result<PageSession> {
        let tab = self.inner.new_tab().context("Failed to create new tab")?;
        if let Some((width, height)) = self.viewport_size {
            apply_viewport_size(&tab, width, height)?;
        }
        Ok(PageSession {
            inner: tab,
            som_pipeline: Arc::new(Mutex::new(SomPipelineState::default())),
            stable_key_index: Arc::new(Mutex::new(HashMap::new())),
            action_logs: Arc::new(Mutex::new(VecDeque::new())),
            policy_engine: Arc::new(Mutex::new(PolicyEngine::default())),
            policy_approvals: Arc::new(Mutex::new(PolicyApprovalState::default())),
            navigation_epoch: Arc::new(AtomicU64::new(0)),
            public_navigation_attempts: Arc::new(AtomicU64::new(0)),
            public_navigation_lock: Arc::new(Mutex::new(())),
            audit_logger: Arc::new(audit_logger),
            semantic_capture_cache: Arc::new(Mutex::new(SemanticCaptureCache::default())),
            vault: Arc::clone(&self.vault),
            plugin_hooks: Arc::clone(&self.plugin_hooks),
            dom_signature_cache: Arc::new(DOMSignatureCache::new()),
        })
    }

    /// OS process ID of the underlying Chrome process, if it was launched
    /// (not connected to via an existing debugger URL).
    ///
    /// Used by integration tests to simulate a Chrome crash (ISSUE-149).
    pub fn process_id(&self) -> Option<u32> {
        self.inner.get_process_id()
    }

    /// Best-effort probe of whether the underlying Chrome process is still
    /// running (ISSUE-260). Returns `None` when the process ID is unknown
    /// (e.g. connected to an existing debugger URL rather than launched) or
    /// liveness could not be determined; `Some(true)`/`Some(false)`
    /// otherwise.
    ///
    /// Used to distinguish a merely disconnected CDP session (e.g. a single
    /// tab losing its transport, or a transient websocket blip) from an
    /// actually dead Chrome process before tearing down the whole browser
    /// session.
    pub fn is_process_alive(&self) -> Option<bool> {
        is_pid_alive(self.process_id()?)
    }

    /// Relaunches the Chrome process after a crash/disconnect and returns a
    /// fresh [`PageSession`] (ISSUE-149: Chrome crash/disconnect recovery).
    ///
    /// Reuses the original `chrome_path` and `viewport_size` so the new
    /// process matches the original launch configuration. The
    /// [`SessionVault`] and plugin hooks are shared (`Arc`) and therefore
    /// survive the relaunch unchanged; in-page navigation state, cookies
    /// outside the vault, and DOM-level recovery caches are reset because the
    /// returned `PageSession` is newly constructed.
    ///
    /// Emits an [`AuditEvent::BrowserRestart`] via `audit_logger` before
    /// returning the new session.
    pub fn relaunch(
        &mut self,
        audit_logger: AuditLogger,
        reason: &str,
        restart_count: u64,
    ) -> Result<PageSession> {
        let mut builder = LaunchOptions::default_builder();
        builder.headless(true);
        if let Some(path) = self.chrome_path.as_ref() {
            builder.path(Some(path.clone().into()));
        }
        if let Some((width, height)) = self.viewport_size {
            builder.window_size(Some((width, height)));
        }
        let options = builder.build().context("Failed to build launch options")?;

        self.inner = Browser::new(options).context("Failed to relaunch browser")?;

        let page = self.new_page_with_audit_logger(audit_logger)?;
        page.audit_logger.log(AuditEvent::BrowserRestart {
            reason: reason.to_string(),
            restart_count,
            timestamp: epoch_millis_u64(),
        });
        Ok(page)
    }

    /// Confirms whether the underlying Chrome process is truly unresponsive
    /// by issuing a lightweight, session-independent `Browser.getVersion`
    /// CDP call bounded by `timeout` (ISSUE-261).
    ///
    /// [`is_browser_disconnected`] matches error-chain markers ("broken
    /// pipe", "connection reset", ...) that can also appear when a
    /// *client-side* request timeout fires on a slow-but-legitimate
    /// operation (e.g. `get_visual`'s screenshot + SoM pipeline keeping the
    /// CDP transport busy) rather than an actual Chrome crash. This
    /// browser-level call is independent of any specific `Tab`'s session,
    /// so it can succeed even while a particular tab's command channel
    /// appears stuck. Callers should treat a suspected disconnect as
    /// confirmed only when this also returns `false`; otherwise the
    /// original error should be propagated instead of triggering
    /// [`relaunch`](Self::relaunch).
    ///
    /// Bounded independently of headless_chrome's internal 30s
    /// `idle_browser_timeout` default via a detached probe thread and a
    /// bounded channel receive: if the probe truly hangs past `timeout`,
    /// its thread is intentionally left to run to completion in the
    /// background (holding a cheap `Arc` clone of the browser handle)
    /// rather than force-killed, since Rust has no safe thread-cancellation
    /// primitive; each suspected disconnect spawns at most one such thread.
    pub fn confirm_alive(&self, timeout: Duration) -> bool {
        let browser = self.inner.clone();
        confirm_alive_with(move || browser.get_version().is_ok(), timeout)
    }
}

/// Test seam for [`BrowserClient::confirm_alive`]: runs `probe` on a
/// background thread and returns its result if it completes within
/// `timeout`, otherwise `false` (ISSUE-261).
pub(super) fn confirm_alive_with(
    probe: impl FnOnce() -> bool + Send + 'static,
    timeout: Duration,
) -> bool {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        // The receiver may already be gone if `rx.recv_timeout` below timed
        // out first; ignore the send failure rather than panicking.
        let _ = tx.send(probe());
    });
    rx.recv_timeout(timeout).unwrap_or(false)
}

/// Transport-level error markers (ISSUE-260): text patterns that most often
/// indicate a transient CDP WebSocket blip (e.g. a large `get_visual`
/// payload write racing a read timeout under load) rather than the Chrome
/// process itself having died.
pub(super) const TRANSPORT_ERROR_MARKERS: [&str; 2] = ["broken pipe", "connection reset"];

/// Returns `true` if `err`'s chain matches a transport-level marker
/// (ISSUE-260). A `true` result does not by itself confirm that the Chrome
/// process died; callers should retry the failed CDP call once before
/// escalating to [`is_browser_disconnected`] / a full restart.
pub fn is_transport_error(err: &anyhow::Error) -> bool {
    error_chain_contains_any(err, &TRANSPORT_ERROR_MARKERS)
}

/// Returns `true` if `err`'s error chain indicates that the underlying Chrome
/// process disconnected (crashed, was killed, or the CDP websocket dropped),
/// as opposed to a page-level error (ISSUE-149) or a merely transient
/// transport blip (ISSUE-260).
///
/// This marker match alone is not sufficient evidence of a genuine crash: a
/// client-side request timeout on a slow-but-alive operation can surface the
/// same transport-level wording (ISSUE-261). Callers that intend to restart
/// the browser on a match should first confirm true unresponsiveness with
/// [`BrowserClient::confirm_alive`] rather than restarting on this signal
/// alone.
pub fn is_browser_disconnected(err: &anyhow::Error) -> bool {
    if err.chain().any(|source| {
        source
            .downcast_ref::<headless_chrome::browser::ConnectionClosed>()
            .is_some()
    }) {
        return true;
    }

    let definite_markers = ["connection is closed", "broken pipe", "connection reset"];
    if error_chain_contains_any(err, &definite_markers) {
        return true;
    }

    // ISSUE-260: "not connected" alone is ambiguous -- it can mean a single
    // tab lost its CDP session while Chrome itself is still alive. Only
    // treat it as a full disconnect when corroborated by a second marker
    // (or literal "ConnectionClosed" text that didn't downcast above,
    // e.g. because it was flattened into a plain string by an intermediate
    // `.context()`).
    if error_chain_contains_any(err, &["not connected"]) {
        let corroborating_markers = [
            "connection is closed",
            "broken pipe",
            "connection reset",
            "connectionclosed",
        ];
        return error_chain_contains_any(err, &corroborating_markers);
    }

    false
}

/// Best-effort liveness probe for a Chrome process by PID (ISSUE-260).
///
/// Returns `Some(true)`/`Some(false)` when liveness could be determined,
/// `None` when it could not (e.g. unsupported platform, or the `ps`
/// utility failed to spawn). Callers should treat `None` the same as
/// "unknown" and fall back to the pre-existing (safe) restart behavior.
///
/// Beyond mere existence, this cross-checks the process name against
/// `"chrom"` (matches "Chrome"/"Chromium" variants) via `ps -o comm=`: a
/// bare existence check (e.g. `kill -0`) only proves *some* process holds
/// that PID -- after Chrome exits, the OS can reuse the PID for an
/// unrelated process, and a pure existence check would then wrongly report
/// the (dead) Chrome as alive.
///
/// A killed-but-not-yet-reaped process becomes a zombie (`ps` state `Z`):
/// the PID and `comm` name are still present, but the process cannot
/// execute anything (including serve CDP requests) -- so a zombie must be
/// treated as dead here, not alive.
#[cfg(unix)]
pub(super) fn is_pid_alive(pid: u32) -> Option<bool> {
    let output = std::process::Command::new("ps")
        .arg("-p")
        .arg(pid.to_string())
        .arg("-o")
        .arg("stat=,comm=")
        .output()
        .ok()?;
    if !output.status.success() {
        return Some(false);
    }
    let line = String::from_utf8_lossy(&output.stdout);
    let Some((stat, comm)) = line.trim().split_once(char::is_whitespace) else {
        return Some(false);
    };
    if stat.starts_with('Z') {
        return Some(false);
    }
    Some(comm.to_lowercase().contains("chrom"))
}

#[cfg(not(unix))]
pub(super) fn is_pid_alive(_pid: u32) -> Option<bool> {
    None
}

pub(super) fn apply_viewport_size(
    tab: &headless_chrome::Tab,
    width: u32,
    height: u32,
) -> Result<()> {
    use headless_chrome::protocol::cdp::Emulation::SetDeviceMetricsOverride;

    tab.call_method(SetDeviceMetricsOverride {
        width,
        height,
        device_scale_factor: 1.0,
        mobile: false,
        scale: None,
        screen_width: Some(width),
        screen_height: Some(height),
        position_x: None,
        position_y: None,
        dont_set_visible_size: None,
        screen_orientation: None,
        viewport: None,
        display_feature: None,
        device_posture: None,
    })
    .context("Failed to set viewport dimensions")?;
    Ok(())
}
