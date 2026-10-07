mod act;
mod audit;
mod capture;
mod helpers;
mod navigation;
mod page_navigation;
mod policy;
mod vault;
mod visual;
use helpers::{
    build_semantic_path_index, collect_stable_key_entries, duration_to_millis, epoch_millis,
    epoch_millis_u64, error_chain_contains_any, find_node_by_id, find_node_by_key,
    is_transient_capture_error, navigation_fallback_condition_met, normalize_dirty_paths,
    normalize_text, quad_to_bbox, remaining_timeout, sleep_transient_backoff,
    state_contains_intent, target_matches_state, transient_error_backoff, value_to_string_vec,
    value_to_u64,
};
pub use navigation::{
    validate_public_navigation_url, validate_public_navigation_url_with, NavigationNetworkPolicy,
    NavigationValidationError, ValidatedNavigationUrl, MAX_PUBLIC_NAVIGATION_URL_BYTES,
};

use anyhow::{Context, Result};
use headless_chrome::{Browser, LaunchOptions};
use std::{
    cmp::min,
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    audit::{AuditEvent, AuditLogger},
    dom_signature::DOMSignatureCache,
    error::{ActionError, VerifyError, WaitError},
    plugin_hooks::{run_policy_hooks, run_state_hooks, PluginHookConfig, PolicyHookOutcome},
    policy::{
        ApprovalScope, OutcomeProjection, PolicyAction, PolicyContext, PolicyEngine, PolicyRule,
    },
    session_vault::{LocalSessionVault, SessionVault, SoftwareKms},
    sre::{
        normalize_dom_with_viewport, normalize_dom_with_viewport_and_refinement, LoadProfile,
        SemanticNode, SemanticState, SubtreeRefinementConfig, ViewportDimensions,
    },
};
// ISSUE-210: only used by the `session-vault-api`-gated save_to_vault/load_from_vault below.
#[cfg(feature = "session-vault-api")]
use crate::session_vault::{CookieData, SessionData};

const DEFAULT_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_TRANSIENT_ERROR_BACKOFF: Duration = Duration::from_millis(250);
const NAVIGATION_FALLBACK_TIMEOUT: Duration = Duration::from_secs(3);
const NAVIGATION_FALLBACK_POLL_INTERVAL: Duration = Duration::from_millis(50);
const SRE_EVENT_BRIDGE_SYMBOL: &str = "neural_browser.runtime.sre_event_bridge";
const ACTION_LOG_BUFFER_LIMIT: usize = 256;
/// Default bound for [`BrowserClient::confirm_alive`]'s CDP liveness probe
/// (ISSUE-261), decoupled from headless_chrome's internal 30s
/// `idle_browser_timeout` default so a suspected-disconnect confirmation
/// stays fast.
pub const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticTarget {
    Id(i64),
    StableKey(String),
    IdWithStableKey { id: i64, stable_key: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticWaitState {
    Enabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticWaitOptions {
    pub load_profile: LoadProfile,
    /// Backoff interval used when transient capture errors occur while waiting.
    /// Zero values fall back to `DEFAULT_WAIT_POLL_INTERVAL`.
    pub poll_interval: Duration,
}

impl Default for SemanticWaitOptions {
    fn default() -> Self {
        Self {
            load_profile: LoadProfile::Minimal,
            poll_interval: DEFAULT_WAIT_POLL_INTERVAL,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SomTrigger {
    GetVisual,
    ActAmbiguous,
    VerifyFailed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SomMark {
    pub id: i64,
    pub stable_key: Option<String>,
    /// `[x, y, width, height]` in CSS pixels.
    pub bbox: [f64; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct VisualCapture {
    pub trigger: SomTrigger,
    pub marks: Vec<SomMark>,
    pub image_png: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionLogEntry {
    pub level: String,
    pub code: String,
    pub action: String,
    pub target_id: Option<i64>,
    pub stable_key: Option<String>,
    pub message: String,
    pub timestamp: u64,
}

#[derive(Default)]
struct SomPipelineState {
    generation_count: usize,
    last_capture: Option<VisualCapture>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StableKeyIndexEntry {
    backend_node_id: i64,
    alias: Option<String>,
}

// PartialEq only (not Eq): `outcome` carries `OutcomeProjection`, which holds an
// f64 and therefore cannot implement Eq.
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyApprovalRequest {
    pub rule_id: String,
    pub scope: ApprovalScope,
    pub action: String,
    pub target_signature: String,
    /// Guardian Angel: structured outcome projection captured when the request
    /// was raised. Surfaced to HITL bridges (e.g. Slack/Teams) alongside the
    /// approval prompt so reviewers see the projected impact.
    pub outcome: Option<OutcomeProjection>,
}

#[derive(Debug, Clone, PartialEq)]
struct GrantedPolicyApproval {
    request: PolicyApprovalRequest,
    granted_navigation_epoch: u64,
    /// URL at the time of approval grant. Used to detect click-driven navigations
    /// that bypass `navigate()` and thus do not increment `navigation_epoch`.
    granted_url: String,
    expires_at_epoch_ms: Option<u128>,
    remaining_uses: Option<u32>,
}

#[derive(Default)]
struct PolicyApprovalState {
    pending: Option<PolicyApprovalRequest>,
    granted: Vec<GrantedPolicyApproval>,
}

#[derive(Debug, Clone)]
enum NavigationPolicyFailure {
    Blocked {
        rule_id: String,
    },
    HumanApprovalRequired {
        rule_id: String,
        scope: ApprovalScope,
        outcome: Option<OutcomeProjection>,
    },
    Rejected {
        message: String,
    },
}

impl NavigationPolicyFailure {
    fn into_error(self) -> anyhow::Error {
        match self {
            Self::Blocked { rule_id } => ActionError::Blocked { rule_id }.into(),
            Self::HumanApprovalRequired {
                rule_id,
                scope,
                outcome,
            } => ActionError::HumanApprovalRequired {
                rule_id,
                scope,
                outcome,
            }
            .into(),
            Self::Rejected { message } => anyhow::anyhow!(message),
        }
    }
}

struct RedirectInterceptionState {
    main_frame_id: String,
    initial_url: String,
    initial_request_seen: bool,
    original_url: String,
    network_policy: NavigationNetworkPolicy,
    request_chain: HashSet<String>,
    evaluated_destinations: HashSet<String>,
    failure: Option<NavigationPolicyFailure>,
}

struct NavigationPolicyContext {
    policy_engine: Arc<Mutex<PolicyEngine>>,
    policy_approvals: Arc<Mutex<PolicyApprovalState>>,
    navigation_epoch: Arc<AtomicU64>,
    audit_logger: Arc<AuditLogger>,
    plugin_hooks: Arc<PluginHookConfig>,
    approval_context_url: String,
}

#[derive(Clone, Default)]
struct SemanticCaptureCache {
    profile: Option<LoadProfile>,
    last_state: Option<Arc<SemanticState>>,
}

pub struct BrowserClient {
    inner: Browser,
    vault: Arc<dyn SessionVault>,
    plugin_hooks: Arc<PluginHookConfig>,
    viewport_size: Option<(u32, u32)>,
    chrome_path: Option<String>,
}

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

    fn new_with_vault_and_path(
        vault: Arc<dyn SessionVault>,
        chrome_path: Option<String>,
    ) -> Result<Self> {
        Self::new_with_vault_and_path_and_hooks(vault, chrome_path, PluginHookConfig::default())
    }

    fn new_with_vault_and_path_and_hooks(
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

    fn new_with_vault_and_size(
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
fn confirm_alive_with(probe: impl FnOnce() -> bool + Send + 'static, timeout: Duration) -> bool {
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
const TRANSPORT_ERROR_MARKERS: [&str; 2] = ["broken pipe", "connection reset"];

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
fn is_pid_alive(pid: u32) -> Option<bool> {
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
fn is_pid_alive(_pid: u32) -> Option<bool> {
    None
}

fn apply_viewport_size(tab: &headless_chrome::Tab, width: u32, height: u32) -> Result<()> {
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

pub struct PageSession {
    inner: Arc<headless_chrome::Tab>,
    som_pipeline: Arc<Mutex<SomPipelineState>>,
    stable_key_index: Arc<Mutex<HashMap<String, StableKeyIndexEntry>>>,
    action_logs: Arc<Mutex<VecDeque<ActionLogEntry>>>,
    policy_engine: Arc<Mutex<PolicyEngine>>,
    policy_approvals: Arc<Mutex<PolicyApprovalState>>,
    navigation_epoch: Arc<AtomicU64>,
    public_navigation_attempts: Arc<AtomicU64>,
    public_navigation_lock: Arc<Mutex<()>>,
    pub(crate) audit_logger: Arc<AuditLogger>,
    semantic_capture_cache: Arc<Mutex<SemanticCaptureCache>>,
    // ISSUE-210: every BrowserClient constructs a real vault regardless (it's required
    // plumbing, not optional), but nothing reads it back unless `session-vault-api` is enabled.
    #[cfg_attr(not(feature = "session-vault-api"), allow(dead_code))]
    vault: Arc<dyn SessionVault>,
    plugin_hooks: Arc<PluginHookConfig>,
    /// Self-Healing Context Recovery cache (PR-21 / ISSUE-11).
    dom_signature_cache: Arc<DOMSignatureCache>,
}

impl PageSession {
    pub fn evaluate_script(
        &self,
        script: &str,
    ) -> Result<headless_chrome::protocol::cdp::Runtime::RemoteObject> {
        self.inner
            .evaluate(script, false)
            .context("Failed to evaluate script")
    }

    /// Evaluate a JS expression and return its value as a JSON `Value`.
    /// Uses `return_by_value: true` so arrays and objects are fully serialized.
    pub fn evaluate_script_json(&self, script: &str) -> Result<serde_json::Value> {
        self.evaluate_script_value(script, false)
    }

    fn evaluate_script_value(
        &self,
        script: &str,
        await_promise: bool,
    ) -> Result<serde_json::Value> {
        use headless_chrome::protocol::cdp::Runtime::Evaluate;

        let result = self
            .inner
            .call_method(Evaluate {
                expression: script.to_string(),
                return_by_value: Some(true),
                generate_preview: Some(false),
                silent: Some(true),
                await_promise: Some(await_promise),
                include_command_line_api: Some(false),
                user_gesture: Some(false),
                object_group: None,
                context_id: None,
                throw_on_side_effect: None,
                timeout: None,
                disable_breaks: None,
                repl_mode: None,
                allow_unsafe_eval_blocked_by_csp: None,
                unique_context_id: None,
                serialization_options: None,
            })
            .context("Failed to evaluate script value")?;

        result
            .result
            .value
            .context("Script evaluation did not return a value")
    }
}

struct SreEventSubscriber<'a> {
    session: &'a PageSession,
    profile: LoadProfile,
    last_state: Option<SemanticState>,
    cached_path_index: HashMap<String, Vec<usize>>,
    last_event_version: u64,
    initial_snapshot_emitted: bool,
}

impl<'a> SreEventSubscriber<'a> {
    fn new(session: &'a PageSession, profile: LoadProfile) -> Self {
        Self {
            session,
            profile,
            last_state: None,
            cached_path_index: HashMap::new(),
            last_event_version: 0,
            initial_snapshot_emitted: false,
        }
    }

    fn wait_next_state_event(&mut self, max_wait: Duration) -> Result<Option<SemanticState>> {
        if !self.initial_snapshot_emitted {
            self.initial_snapshot_emitted = true;
            return self.capture_next_state_if_changed(Vec::new());
        }

        let bridge_version = self.session.ensure_sre_event_bridge()?;
        if bridge_version != self.last_event_version {
            self.last_event_version = bridge_version;
            let dirty_paths = self.session.take_sre_dirty_paths()?;
            return self.capture_next_state_if_changed(dirty_paths);
        }

        let next_version = self
            .session
            .wait_for_sre_event(self.last_event_version, max_wait)?;
        if next_version == self.last_event_version {
            return Ok(None);
        }

        self.last_event_version = next_version;
        let dirty_paths = self.session.take_sre_dirty_paths()?;
        self.capture_next_state_if_changed(dirty_paths)
    }

    fn capture_next_state_if_changed(
        &mut self,
        dirty_paths: Vec<String>,
    ) -> Result<Option<SemanticState>> {
        let state = self.session.capture_state_with_refinement(
            self.profile,
            &dirty_paths,
            self.last_state.as_ref().map(SemanticState::root),
            Some(&self.cached_path_index),
        )?;
        let shared_state = Arc::new(state.clone());
        self.session.record_state_update(
            self.last_state
                .as_ref()
                .map(|state| Arc::new(state.clone())),
            Arc::clone(&shared_state),
        )?;

        let is_unchanged_hash = self
            .last_state
            .as_ref()
            .is_some_and(|previous| previous.state_hash() == state.state_hash());

        self.cached_path_index = build_semantic_path_index(state.root());
        self.last_state = Some(state.clone());

        if is_unchanged_hash {
            return Ok(None);
        }
        Ok(Some(state))
    }
}

fn resolve_policy_target_node<'a>(
    root: &'a SemanticNode,
    target_id: Option<i64>,
    stable_key: Option<&str>,
) -> Option<&'a SemanticNode> {
    if let Some(id) = target_id {
        if let Some(node) = find_node_by_id(root, id) {
            return Some(node);
        }
    }

    if let Some(key) = stable_key {
        if let Some(node) = find_node_by_key(root, key) {
            return Some(node);
        }
    }

    None
}

/// Find the immediate parent node of `target` in the semantic tree rooted at `root`.
fn find_parent_of_node<'a>(
    root: &'a SemanticNode,
    target: &SemanticNode,
) -> Option<&'a SemanticNode> {
    for child in &root.children {
        if std::ptr::eq(child as *const _, target as *const _) {
            return Some(root);
        }
        if let Some(found) = find_parent_of_node(child, target) {
            return Some(found);
        }
    }
    None
}

/// Collect surrounding context text for policy evaluation.
///
/// In addition to the target node's own label and attributes, this walks the
/// parent container and collects visible text from sibling nodes so that
/// `context_regex` rules (e.g. matching `Total: $149` outside a button) work
/// correctly for realistic checkout DOM structures.
fn policy_context_text(node: &SemanticNode, root: &SemanticNode) -> String {
    let mut parts = Vec::new();

    // Target node: label + attribute values
    push_policy_text_part(&mut parts, node.label.as_deref());
    if let Some(attrs) = &node.attributes {
        for value in attrs.values() {
            push_policy_text_part(&mut parts, Some(value.as_str()));
        }
    }

    // Parent container: label and direct children's text
    if let Some(parent) = find_parent_of_node(root, node) {
        push_policy_text_part(&mut parts, parent.label.as_deref());
        for sibling in &parent.children {
            // Collect text-role siblings and their descendant text
            collect_policy_descendant_text(sibling, &mut parts);
        }
    }

    parts.join(" ")
}

fn policy_target_text(node: &SemanticNode) -> Option<String> {
    let mut parts = Vec::new();
    push_policy_text_part(&mut parts, node.label.as_deref());
    collect_policy_descendant_text(node, &mut parts);

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

fn collect_policy_descendant_text(node: &SemanticNode, parts: &mut Vec<String>) {
    for child in &node.children {
        if child.role == "text" {
            push_policy_text_part(parts, child.label.as_deref());
        }
        collect_policy_descendant_text(child, parts);
    }
}

fn push_policy_text_part(parts: &mut Vec<String>, value: Option<&str>) {
    let Some(normalized) = value
        .map(str::trim)
        .filter(|candidate| !candidate.is_empty())
    else {
        return;
    };

    if parts.iter().any(|existing| existing == normalized) {
        return;
    }

    parts.push(normalized.to_string());
}

type NavigationRequestInterceptor =
    dyn headless_chrome::browser::tab::RequestInterceptor + Send + Sync;

trait NavigationInterceptionControl: Send + Sync {
    fn install_interceptor(&self, interceptor: Arc<NavigationRequestInterceptor>) -> Result<()>;
    fn enable_fetch(
        &self,
        patterns: &[headless_chrome::protocol::cdp::Fetch::RequestPattern],
    ) -> Result<()>;
    fn restore_default_interceptor(&self) -> Result<()>;
    fn disable_fetch(&self) -> Result<()>;
}

struct TabNavigationInterceptionControl {
    tab: Arc<headless_chrome::Tab>,
}

impl NavigationInterceptionControl for TabNavigationInterceptionControl {
    fn install_interceptor(&self, interceptor: Arc<NavigationRequestInterceptor>) -> Result<()> {
        self.tab.enable_request_interception(interceptor)
    }

    fn enable_fetch(
        &self,
        patterns: &[headless_chrome::protocol::cdp::Fetch::RequestPattern],
    ) -> Result<()> {
        self.tab.enable_fetch(Some(patterns), None).map(|_| ())
    }

    fn restore_default_interceptor(&self) -> Result<()> {
        self.tab.enable_request_interception(Arc::new(
            |_: Arc<headless_chrome::browser::transport::Transport>,
             _: headless_chrome::browser::transport::SessionId,
             _: headless_chrome::protocol::cdp::Fetch::events::RequestPausedEvent| {
                headless_chrome::browser::tab::RequestPausedDecision::Continue(None)
            },
        ))
    }

    fn disable_fetch(&self) -> Result<()> {
        self.tab.disable_fetch().map(|_| ())
    }
}

struct NavigationInterceptionGuard {
    control: Arc<dyn NavigationInterceptionControl>,
    active: bool,
}

impl NavigationInterceptionGuard {
    fn install<F>(
        tab: Arc<headless_chrome::Tab>,
        interceptor: Arc<F>,
        patterns: &[headless_chrome::protocol::cdp::Fetch::RequestPattern],
    ) -> Result<Self>
    where
        F: headless_chrome::browser::tab::RequestInterceptor + Send + Sync + 'static,
    {
        Self::install_with_control(
            Arc::new(TabNavigationInterceptionControl { tab }),
            interceptor,
            patterns,
        )
    }

    fn install_with_control<F>(
        control: Arc<dyn NavigationInterceptionControl>,
        interceptor: Arc<F>,
        patterns: &[headless_chrome::protocol::cdp::Fetch::RequestPattern],
    ) -> Result<Self>
    where
        F: headless_chrome::browser::tab::RequestInterceptor + Send + Sync + 'static,
    {
        let guard = Self {
            control,
            active: true,
        };
        let interceptor: Arc<NavigationRequestInterceptor> = interceptor;
        guard
            .control
            .install_interceptor(interceptor)
            .context("failed to install public navigation interceptor")?;
        guard
            .control
            .enable_fetch(patterns)
            .context("failed to enable public navigation interception")?;
        Ok(guard)
    }

    fn finish(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        let restore_result = self
            .control
            .restore_default_interceptor()
            .context("failed to restore default request interceptor");
        let disable_result = self
            .control
            .disable_fetch()
            .context("failed to disable public navigation interception");
        restore_result.and(disable_result)
    }
}

impl Drop for NavigationInterceptionGuard {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

fn evaluate_navigation_destination(
    context: &NavigationPolicyContext,
    destination: &ValidatedNavigationUrl,
    approval_signature: &str,
    clear_pending_on_block: bool,
) -> std::result::Result<(), NavigationPolicyFailure> {
    let decision = context
        .policy_engine
        .lock()
        .map_err(|_| NavigationPolicyFailure::Rejected {
            message: "navigation policy engine is unavailable".to_string(),
        })?
        .evaluate(&PolicyContext {
            url: destination.canonical_url().to_string(),
            action: "navigate".to_string(),
            target_role: None,
            target_text: None,
            surrounding_text: None,
        });
    let rule_id = decision
        .rule_id
        .clone()
        .unwrap_or_else(|| "unnamed-policy-rule".to_string());
    context.audit_logger.log(AuditEvent::PolicyDecision {
        rule_id: rule_id.clone(),
        action: "navigate".to_string(),
        decision: match decision.action {
            PolicyAction::Allow => "allow",
            PolicyAction::Block => "block",
            PolicyAction::RequireHumanApproval => "require_human_approval",
        }
        .to_string(),
        destination_fingerprint: Some(format!("sha256:{}", destination.destination_digest())),
        timestamp: epoch_millis_u64(),
    });

    let run_plugins = || {
        let intent = serde_json::json!({
            "action": "navigate",
            "url": destination.canonical_url(),
        })
        .to_string();
        let (outcome, events) = run_policy_hooks(
            &intent,
            context
                .plugin_hooks
                .policy_plugins
                .iter()
                .map(|p| p.as_ref()),
        );
        // Plugin reasons are untrusted and may echo the full destination URL.
        // Keep attribution and allow/block result, but never persist the reason.
        for event in events {
            if let AuditEvent::PluginPolicyDecision {
                plugin_id,
                allowed,
                timestamp,
                ..
            } = event
            {
                context.audit_logger.log(AuditEvent::PluginPolicyDecision {
                    plugin_id,
                    allowed,
                    reason: None,
                    timestamp,
                });
            }
        }
        match outcome {
            PolicyHookOutcome::Allow => Ok(()),
            PolicyHookOutcome::Block { plugin_id, .. } => Err(NavigationPolicyFailure::Blocked {
                rule_id: format!("plugin:{plugin_id}"),
            }),
        }
    };

    match decision.action {
        PolicyAction::Allow => run_plugins(),
        PolicyAction::Block => {
            if clear_pending_on_block {
                if let Ok(mut approvals) = context.policy_approvals.lock() {
                    approvals.pending = None;
                }
            }
            Err(NavigationPolicyFailure::Blocked { rule_id })
        }
        PolicyAction::RequireHumanApproval => {
            let scope = decision.scope.unwrap_or(ApprovalScope::ActionOnly);
            if consume_navigation_approval(context, &rule_id, scope, approval_signature) {
                return run_plugins();
            }
            let request = PolicyApprovalRequest {
                rule_id: rule_id.clone(),
                scope,
                action: "navigate".to_string(),
                target_signature: approval_signature.to_string(),
                outcome: decision.outcome.clone(),
            };
            context
                .policy_approvals
                .lock()
                .map_err(|_| NavigationPolicyFailure::Rejected {
                    message: "navigation approval state is unavailable".to_string(),
                })?
                .pending = Some(request);
            context.audit_logger.log(AuditEvent::HitlEvent {
                event_type: "request".to_string(),
                reason: Some(format!("Requires human approval for rule {rule_id}")),
                user_id: None,
                timestamp: epoch_millis_u64(),
            });
            Err(NavigationPolicyFailure::HumanApprovalRequired {
                rule_id,
                scope,
                outcome: decision.outcome,
            })
        }
    }
}

fn consume_navigation_approval(
    context: &NavigationPolicyContext,
    rule_id: &str,
    scope: ApprovalScope,
    approval_signature: &str,
) -> bool {
    let now_ms = epoch_millis();
    let navigation_epoch = context.navigation_epoch.load(Ordering::Relaxed);
    let Ok(mut approvals) = context.policy_approvals.lock() else {
        return false;
    };
    approvals.granted.retain(|grant| {
        is_grant_valid(
            grant,
            navigation_epoch,
            &context.approval_context_url,
            now_ms,
        )
    });
    let Some(index) = approvals.granted.iter().position(|grant| {
        grant.request.rule_id == rule_id
            && grant.request.scope == scope
            && grant.request.action == "navigate"
            && grant.request.target_signature == approval_signature
    }) else {
        return false;
    };
    if scope == ApprovalScope::ActionOnly {
        approvals.granted.remove(index);
    }
    true
}

fn policy_target_signature(
    node: Option<&SemanticNode>,
    target_id: Option<i64>,
    stable_key: Option<&str>,
) -> String {
    if let Some(key) = node.and_then(|resolved| resolved.stable_key.as_deref()) {
        return key.to_string();
    }

    if let Some(key) = stable_key {
        let normalized = key.trim();
        if !normalized.is_empty() {
            return normalized.to_string();
        }
    }

    if let Some(id) = node.map(|resolved| resolved.backend_node_id).or(target_id) {
        return format!("backend_node_id:{id}");
    }

    "unknown-target".to_string()
}

fn is_grant_valid(
    grant: &GrantedPolicyApproval,
    navigation_epoch: u64,
    current_url: &str,
    now_ms: u128,
) -> bool {
    match grant.request.scope {
        ApprovalScope::ActionOnly => {
            grant.granted_navigation_epoch == navigation_epoch
                && grant.remaining_uses.unwrap_or(0) > 0
        }
        // UntilNavigation expires when either the explicit navigate() increments the epoch
        // OR the URL changed due to a click-driven navigation (which does not increment epoch).
        ApprovalScope::UntilNavigation => {
            grant.granted_navigation_epoch == navigation_epoch && grant.granted_url == current_url
        }
        ApprovalScope::Timeboxed { .. } => grant
            .expires_at_epoch_ms
            .is_some_and(|expires_at| now_ms <= expires_at),
    }
}

/// Number of hex characters exposed in the external (LLM-facing) stable_key.
/// 16 hex chars = 64 bits of entropy; negligible collision probability at page scale.
/// Re-exported from `core-runtime` so downstream crates share a single source of truth.
pub const STABLE_KEY_SHORT_LEN: usize = 16;

#[cfg(test)]
mod tests {
    use super::*;
    use helpers::{node_is_enabled, normalized_poll_interval};

    #[derive(Default)]
    struct RecordingInterceptionControl {
        operations: Mutex<Vec<&'static str>>,
        fail_at: Option<&'static str>,
    }

    impl RecordingInterceptionControl {
        fn failing_at(operation: &'static str) -> Self {
            Self {
                operations: Mutex::new(Vec::new()),
                fail_at: Some(operation),
            }
        }

        fn record(&self, operation: &'static str) -> Result<()> {
            self.operations.lock().unwrap().push(operation);
            if self.fail_at == Some(operation) {
                anyhow::bail!("injected {operation} failure");
            }
            Ok(())
        }

        fn recorded(&self) -> Vec<&'static str> {
            self.operations.lock().unwrap().clone()
        }
    }

    impl NavigationInterceptionControl for RecordingInterceptionControl {
        fn install_interceptor(
            &self,
            _interceptor: Arc<NavigationRequestInterceptor>,
        ) -> Result<()> {
            self.record("install_interceptor")
        }

        fn enable_fetch(
            &self,
            _patterns: &[headless_chrome::protocol::cdp::Fetch::RequestPattern],
        ) -> Result<()> {
            self.record("enable_fetch")
        }

        fn restore_default_interceptor(&self) -> Result<()> {
            self.record("restore_default_interceptor")
        }

        fn disable_fetch(&self) -> Result<()> {
            self.record("disable_fetch")
        }
    }

    fn recording_interceptor() -> Arc<impl headless_chrome::browser::tab::RequestInterceptor> {
        Arc::new(
            |_: Arc<headless_chrome::browser::transport::Transport>,
             _: headless_chrome::browser::transport::SessionId,
             _: headless_chrome::protocol::cdp::Fetch::events::RequestPausedEvent| {
                headless_chrome::browser::tab::RequestPausedDecision::Continue(None)
            },
        )
    }

    #[test]
    fn navigation_interception_partial_enable_failure_restores_and_disables() {
        let control = Arc::new(RecordingInterceptionControl::failing_at("enable_fetch"));
        let result = NavigationInterceptionGuard::install_with_control(
            control.clone(),
            recording_interceptor(),
            &[],
        );

        assert!(result.is_err());
        assert_eq!(
            control.recorded(),
            vec![
                "install_interceptor",
                "enable_fetch",
                "restore_default_interceptor",
                "disable_fetch"
            ]
        );
    }

    #[test]
    fn navigation_interception_success_finishes_restore_before_disable_once() {
        let control = Arc::new(RecordingInterceptionControl::default());
        let mut guard = NavigationInterceptionGuard::install_with_control(
            control.clone(),
            recording_interceptor(),
            &[],
        )
        .expect("install interception");

        guard.finish().expect("finish interception");
        guard.finish().expect("second finish is idempotent");
        drop(guard);

        assert_eq!(
            control.recorded(),
            vec![
                "install_interceptor",
                "enable_fetch",
                "restore_default_interceptor",
                "disable_fetch"
            ]
        );
    }

    #[test]
    fn navigation_interception_finish_attempts_disable_after_cleanup_errors() {
        for failure in ["restore_default_interceptor", "disable_fetch"] {
            let control = Arc::new(RecordingInterceptionControl::failing_at(failure));
            let mut guard = NavigationInterceptionGuard::install_with_control(
                control.clone(),
                recording_interceptor(),
                &[],
            )
            .expect("install interception");

            assert!(guard.finish().is_err(), "failure point: {failure}");
            assert_eq!(
                control.recorded(),
                vec![
                    "install_interceptor",
                    "enable_fetch",
                    "restore_default_interceptor",
                    "disable_fetch"
                ],
                "failure point: {failure}"
            );
        }
    }

    #[test]
    fn test_state_contains_intent_exact_match() {
        let node = SemanticNode {
            role: "body".to_string(),
            attributes: Some(std::collections::BTreeMap::from([(
                "data-intent".to_string(),
                "checkout_complete".to_string(),
            )])),
            ..Default::default()
        };

        assert!(state_contains_intent(&node, "checkout_complete"));
        assert!(!state_contains_intent(&node, "signup_complete"));
    }

    #[test]
    fn test_node_is_enabled_with_disabled_attribute() {
        let disabled = SemanticNode {
            role: "button".to_string(),
            attributes: Some(std::collections::BTreeMap::from([(
                "disabled".to_string(),
                "".to_string(),
            )])),
            ..Default::default()
        };
        let enabled = SemanticNode {
            role: "button".to_string(),
            attributes: Some(std::collections::BTreeMap::from([(
                "id".to_string(),
                "btn_login".to_string(),
            )])),
            ..Default::default()
        };

        assert!(!node_is_enabled(&disabled));
        assert!(node_is_enabled(&enabled));
    }

    #[test]
    fn test_transient_capture_error_detection() {
        let transient = anyhow::anyhow!("Execution context was destroyed while loading");
        let non_transient = anyhow::anyhow!("Unsupported action: drag");
        let closed = anyhow::anyhow!("Target closed");

        assert!(is_transient_capture_error(&transient));
        assert!(!is_transient_capture_error(&non_transient));
        assert!(!is_transient_capture_error(&closed));
    }

    #[test]
    fn test_state_contains_intent_is_exact_match() {
        let node = SemanticNode {
            role: "text".to_string(),
            label: Some("checkout_complete_failed".to_string()),
            ..Default::default()
        };

        assert!(!state_contains_intent(&node, "checkout_complete"));
    }

    #[test]
    fn test_normalized_poll_interval_uses_default_for_zero() {
        assert_eq!(
            normalized_poll_interval(Duration::ZERO),
            DEFAULT_WAIT_POLL_INTERVAL
        );
    }

    #[test]
    fn test_normalized_poll_interval_keeps_non_zero_value() {
        let interval = Duration::from_millis(123);
        assert_eq!(normalized_poll_interval(interval), interval);
    }

    #[test]
    fn test_transient_error_backoff_is_capped() {
        assert_eq!(
            transient_error_backoff(Duration::from_secs(5)),
            MAX_TRANSIENT_ERROR_BACKOFF
        );
        assert_eq!(
            transient_error_backoff(Duration::from_millis(80)),
            Duration::from_millis(80)
        );
    }

    #[test]
    fn test_navigation_fallback_condition_met_when_requested_url_reached() {
        assert!(navigation_fallback_condition_met(
            "https://example.com/next",
            Some("https://example.com/current"),
            Some("https://example.com/next"),
            Some("complete"),
            true,
        ));
    }

    #[test]
    fn test_navigation_fallback_condition_met_when_url_changes_and_dom_available() {
        assert!(navigation_fallback_condition_met(
            "https://example.com/requested",
            Some("https://example.com/current"),
            Some("https://example.com/redirected"),
            None,
            true,
        ));
    }

    #[test]
    fn test_navigation_fallback_condition_not_met_when_url_unchanged() {
        assert!(!navigation_fallback_condition_met(
            "https://example.com/target",
            Some("https://example.com/current"),
            Some("https://example.com/current"),
            Some("complete"),
            true,
        ));
    }

    #[test]
    fn test_navigation_fallback_condition_not_met_without_dom_readiness() {
        assert!(!navigation_fallback_condition_met(
            "https://example.com/requested",
            Some("https://example.com/current"),
            Some("https://example.com/redirected"),
            Some("loading"),
            false,
        ));
    }

    #[test]
    fn test_normalize_dirty_paths_deduplicates_and_trims() {
        let dirty_paths = vec![
            " root/#document/html/body/div ".to_string(),
            "root/#document/html/body/div".to_string(),
            String::new(),
            "   ".to_string(),
        ];

        let normalized = normalize_dirty_paths(&dirty_paths);
        assert_eq!(normalized.len(), 1);
        assert!(normalized.contains("root/#document/html/body/div"));
    }

    #[test]
    fn test_build_semantic_path_index_uses_unique_paths_only() {
        let tree = SemanticNode {
            role: "#document".to_string(),
            children: vec![SemanticNode {
                role: "html".to_string(),
                children: vec![SemanticNode {
                    role: "body".to_string(),
                    children: vec![
                        SemanticNode {
                            role: "button".to_string(),
                            label: Some("first".to_string()),
                            ..Default::default()
                        },
                        SemanticNode {
                            role: "button".to_string(),
                            label: Some("second".to_string()),
                            ..Default::default()
                        },
                        SemanticNode {
                            role: "section".to_string(),
                            children: vec![SemanticNode {
                                role: "input".to_string(),
                                ..Default::default()
                            }],
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let index = build_semantic_path_index(&tree);
        assert_eq!(index.get("root/#document"), Some(&Vec::<usize>::new()));
        assert!(!index.contains_key("root/#document/html/body/button"));
        assert_eq!(
            index.get("root/#document/html/body/section/input"),
            Some(&vec![0, 0, 2, 0])
        );
    }

    #[test]
    fn is_browser_disconnected_detects_connection_closed_error() {
        let err = anyhow::Error::new(headless_chrome::browser::ConnectionClosed {});
        assert!(is_browser_disconnected(&err));
    }

    #[test]
    fn is_browser_disconnected_detects_connection_closed_in_chain() {
        let err = anyhow::Error::new(headless_chrome::browser::ConnectionClosed {})
            .context("Failed to capture semantic state");
        assert!(is_browser_disconnected(&err));
    }

    #[test]
    fn is_browser_disconnected_detects_io_markers() {
        for marker in [
            "Connection is closed",
            "Broken pipe",
            "connection reset by peer",
        ] {
            let err = anyhow::anyhow!("{marker}");
            assert!(
                is_browser_disconnected(&err),
                "expected disconnect marker '{marker}' to be detected"
            );
        }
    }

    #[test]
    fn is_browser_disconnected_ignores_page_level_errors() {
        let err = anyhow::anyhow!("Could not find node with given id");
        assert!(!is_browser_disconnected(&err));
    }

    // ISSUE-260: "not connected" alone is ambiguous (a single tab can lose
    // its CDP session while Chrome itself stays alive), so it must not by
    // itself trigger a full session teardown.
    #[test]
    fn is_browser_disconnected_ignores_bare_not_connected_marker() {
        let err = anyhow::anyhow!("transport not connected");
        assert!(!is_browser_disconnected(&err));
    }

    #[test]
    fn is_browser_disconnected_detects_not_connected_with_corroboration() {
        let err = anyhow::anyhow!("tab not connected: broken pipe while writing frame");
        assert!(is_browser_disconnected(&err));
    }

    // ISSUE-260: "broken pipe" / "connection reset" are transport-level
    // blips that callers should retry before treating as a disconnect.
    #[test]
    fn is_transport_error_detects_broken_pipe_and_connection_reset() {
        for marker in ["broken pipe", "connection reset by peer"] {
            let err = anyhow::anyhow!("{marker}");
            assert!(
                is_transport_error(&err),
                "expected transport marker '{marker}' to be detected"
            );
        }
    }

    #[test]
    fn is_transport_error_ignores_definite_disconnect_markers() {
        let err = anyhow::anyhow!("connection is closed");
        assert!(!is_transport_error(&err));
    }

    #[test]
    fn is_transport_error_ignores_page_level_errors() {
        let err = anyhow::anyhow!("Could not find node with given id");
        assert!(!is_transport_error(&err));
    }

    #[test]
    fn confirm_alive_with_returns_true_when_probe_succeeds_quickly() {
        assert!(confirm_alive_with(|| true, Duration::from_millis(500)));
    }

    #[test]
    fn confirm_alive_with_returns_false_when_probe_fails_quickly() {
        assert!(!confirm_alive_with(|| false, Duration::from_millis(500)));
    }

    #[test]
    fn confirm_alive_with_returns_false_and_respects_bound_when_probe_hangs() {
        // Simulates a genuinely wedged (open-but-unresponsive) CDP call:
        // the probe never returns within the bound, so the health check
        // must report "not confirmed alive" without blocking past
        // `timeout`, independent of any internal transport default.
        let timeout = Duration::from_millis(200);
        let started = Instant::now();
        let alive = confirm_alive_with(
            || {
                thread::sleep(Duration::from_secs(5));
                true
            },
            timeout,
        );
        assert!(!alive);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "confirm_alive_with should not block past its bounded timeout, took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn find_node_by_key_matches_full_64_char_key() {
        let full_key = "a".repeat(64);
        let node = SemanticNode {
            stable_key: Some(full_key.clone()),
            ..Default::default()
        };
        assert!(find_node_by_key(&node, &full_key).is_some());
    }

    #[test]
    fn find_node_by_key_matches_shortened_16_char_key_against_full_key() {
        // get_state now returns only the first 16 chars; enforce_policy must still
        // resolve the node from the SemanticTree which stores full 64-char keys.
        let full_key = "abcdef1234567890".repeat(4); // 64 chars
        let short_key: String = full_key.chars().take(STABLE_KEY_SHORT_LEN).collect();
        let node = SemanticNode {
            stable_key: Some(full_key.clone()),
            ..Default::default()
        };
        assert!(
            find_node_by_key(&node, &short_key).is_some(),
            "shortened key should resolve the node for policy evaluation"
        );
    }

    #[test]
    fn find_node_by_key_does_not_match_wrong_prefix() {
        let full_key = "abcdef1234567890".repeat(4);
        let wrong_short = "0000000000000000";
        let node = SemanticNode {
            stable_key: Some(full_key.clone()),
            ..Default::default()
        };
        assert!(find_node_by_key(&node, wrong_short).is_none());
    }

    #[test]
    fn find_node_by_key_empty_target_key_never_matches() {
        // An empty target_key would produce cmp_len=0 and ""[..0]==""[..0] which
        // is always true — guard against that authorization bypass.
        let node = SemanticNode {
            stable_key: Some("abcdef1234567890".to_string()),
            ..Default::default()
        };
        assert!(
            find_node_by_key(&node, "").is_none(),
            "empty target_key must not match any node"
        );
    }

    #[test]
    fn resolve_policy_target_node_resolves_via_shortened_stable_key() {
        let full_key = "deadbeefcafe0123".repeat(4);
        let short_key: String = full_key.chars().take(STABLE_KEY_SHORT_LEN).collect();

        let child = SemanticNode {
            role: "button".to_string(),
            stable_key: Some(full_key.clone()),
            ..Default::default()
        };
        let root = SemanticNode {
            role: "body".to_string(),
            children: vec![child],
            ..Default::default()
        };

        let found = resolve_policy_target_node(&root, None, Some(&short_key));
        assert!(
            found.is_some(),
            "policy target resolution must work with the 16-char short key returned by get_state"
        );
        assert_eq!(found.unwrap().role, "button");
    }
}

#[cfg(test)]
mod browser_tests {
    use super::*;

    #[test]
    fn test_browser_initialization() {
        // This test requires a browser installed, so we might want to skip it if strictly unit testing logic
        // But for now, let's see if it compiles and runs in the environment
        if std::env::var("CI").is_ok() {
            return; // Skip in CI without browser setup
        }
        let browser = BrowserClient::new();
        assert!(browser.is_ok());
    }

    #[tokio::test]
    async fn test_session_vault_save_load() -> Result<()> {
        if !crate::chrome_available() {
            return Ok(());
        }

        let client = BrowserClient::new()?;
        let page = client.new_page()?;

        page.navigate("https://example.com")?;

        // Set a cookie manually to test save/load
        page.inner
            .call_method(headless_chrome::protocol::cdp::Network::SetCookie {
                name: "test_cookie".to_string(),
                value: "test_value".to_string(),
                url: Some("https://example.com".to_string()),
                domain: None,
                path: None,
                secure: None,
                http_only: None,
                same_site: None,
                expires: None,
                priority: None,
                same_party: None,
                source_scheme: None,
                source_port: None,
                partition_key: None,
            })?;

        // Save to vault
        page.save_to_vault("my-session").await?;

        // Create a new page and load from vault
        let page2 = client.new_page()?;
        page2.navigate("https://example.com")?; // Navigate first so it has the right context

        // Clear existing browser cookies so the restore path is actually exercised.
        page2
            .inner
            .call_method(headless_chrome::protocol::cdp::Network::ClearBrowserCookies(None))?;
        let cookies_before = page2.inner.get_cookies()?;
        let found_before = cookies_before
            .iter()
            .any(|c| c.name == "test_cookie" && c.value == "test_value");
        assert!(!found_before, "Cookie unexpectedly present before restore");

        page2.load_from_vault("my-session").await?;

        // Verify cookie exists in page2
        let cookies = page2.inner.get_cookies()?;
        let found = cookies
            .iter()
            .any(|c| c.name == "test_cookie" && c.value == "test_value");
        assert!(found, "Cookie 'test_cookie' not found in restored session");

        Ok(())
    }
}
