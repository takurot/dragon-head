use anyhow::{Context, Result};
use headless_chrome::{Browser, LaunchOptions};
use std::{
    cmp::min,
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    audit::{AuditEvent, AuditLogger},
    dom_signature::DOMSignatureCache,
    error::{ActionError, VerifyError, WaitError},
    plugin_hooks::{run_policy_hooks, run_state_hooks, PluginHookConfig, PolicyHookOutcome},
    policy::{
        ApprovalScope, OutcomeProjection, PolicyAction, PolicyContext, PolicyEngine, PolicyRule,
    },
    session_vault::{CookieData, LocalSessionVault, SessionData, SessionVault, SoftwareKms},
    sre::{
        normalize_dom_with_viewport, normalize_dom_with_viewport_and_refinement, LoadProfile,
        SemanticNode, SemanticState, SubtreeRefinementConfig, ViewportDimensions,
    },
};

const DEFAULT_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MAX_TRANSIENT_ERROR_BACKOFF: Duration = Duration::from_millis(250);
const NAVIGATION_FALLBACK_TIMEOUT: Duration = Duration::from_secs(3);
const NAVIGATION_FALLBACK_POLL_INTERVAL: Duration = Duration::from_millis(50);
const SRE_EVENT_BRIDGE_SYMBOL: &str = "neural_browser.runtime.sre_event_bridge";
const ACTION_LOG_BUFFER_LIMIT: usize = 256;

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
}

impl BrowserClient {
    pub fn new() -> Result<Self> {
        use rand::RngCore;
        let mut key = [0u8; 32];
        rand::rng().fill_bytes(&mut key);
        let kms = Box::new(SoftwareKms::new(key, "default-key".to_string()));
        let vault = Arc::new(LocalSessionVault::new(kms));
        Self::new_with_vault(vault)
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
        if let Some(path) = chrome_path {
            builder.path(Some(path.into()));
        }
        let options = builder.build().context("Failed to build launch options")?;

        let browser = Browser::new(options).context("Failed to launch browser")?;
        Ok(Self {
            inner: browser,
            vault,
            plugin_hooks: Arc::new(plugin_hooks),
            viewport_size: None,
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
            audit_logger: Arc::new(audit_logger),
            semantic_capture_cache: Arc::new(Mutex::new(SemanticCaptureCache::default())),
            vault: Arc::clone(&self.vault),
            plugin_hooks: Arc::clone(&self.plugin_hooks),
            dom_signature_cache: Arc::new(DOMSignatureCache::new()),
        })
    }
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
    pub(crate) audit_logger: Arc<AuditLogger>,
    semantic_capture_cache: Arc<Mutex<SemanticCaptureCache>>,
    vault: Arc<dyn SessionVault>,
    plugin_hooks: Arc<PluginHookConfig>,
    /// Self-Healing Context Recovery cache (PR-21 / ISSUE-11).
    dom_signature_cache: Arc<DOMSignatureCache>,
}

impl PageSession {
    pub fn action_logs(&self) -> Result<Vec<ActionLogEntry>> {
        let guard = self
            .action_logs
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock action logs"))?;
        Ok(guard.iter().cloned().collect())
    }

    pub fn audit_events(&self) -> Vec<AuditEvent> {
        self.audit_logger.recent_events()
    }

    pub fn clear_audit_events(&self) {
        self.audit_logger.clear_recent_events();
    }

    /// Returns `(events_written, bytes_written)` from the persistent rolling-file sink,
    /// or `None` when no persistent sink is configured (e.g. `AUDIT_LOG_DIR` is unset).
    pub fn persistent_audit_metrics(&self) -> Option<(u64, u64)> {
        self.audit_logger.persistent_metrics()
    }

    pub fn navigate(&self, url: &str) -> Result<()> {
        let previous_url = self.current_url().ok();
        self.inner.navigate_to(url).context("Failed to navigate")?;
        if let Err(wait_error) = self.inner.wait_until_navigated() {
            self.wait_for_navigation_fallback(url, previous_url.as_deref())
                .with_context(|| {
                    format!("Failed to wait for navigation (primary wait error: {wait_error})")
                })?;
        }
        self.clear_stable_key_index();
        self.clear_semantic_capture_cache();
        self.navigation_epoch.fetch_add(1, Ordering::Relaxed);
        self.clear_pending_policy_approval();
        Ok(())
    }

    pub fn get_content(&self) -> Result<String> {
        self.inner
            .get_content()
            .context("Failed to get page content")
    }

    pub fn get_title(&self) -> Result<String> {
        self.inner.get_title().context("Failed to get page title")
    }

    /// Saves the current session (cookies) to the vault with the given session ID.
    pub async fn save_to_vault(&self, session_id: &str) -> Result<()> {
        let cookies = self.inner.get_cookies().context("Failed to get cookies")?;
        let cookie_data: Vec<CookieData> = cookies
            .into_iter()
            .map(|c| CookieData {
                name: c.name,
                value: c.value,
                domain: c.domain,
                path: c.path,
                expires: c.expires,
                size: c.size,
                http_only: c.http_only,
                secure: c.secure,
                session: c.session,
                same_site: c.same_site.map(|s| format!("{:?}", s)),
                priority: format!("{:?}", c.priority),
            })
            .collect();

        let data = SessionData {
            domain: self
                .current_url()
                .context("Failed to resolve current URL while saving session")?,
            cookies: cookie_data,
            tokens: HashMap::new(), // Placeholder for other tokens (e.g. localStorage)
        };

        self.vault.store_session(session_id, &data).await?;
        Ok(())
    }

    /// Loads a session from the vault and restores cookies to the browser.
    pub async fn load_from_vault(&self, session_id: &str) -> Result<()> {
        use headless_chrome::protocol::cdp::Network::{CookiePriority, CookieSameSite};

        if let Some(data) = self.vault.load_session(session_id).await? {
            let session_url = (!data.domain.is_empty()).then_some(data.domain.clone());
            for cookie in data.cookies {
                let same_site = match cookie.same_site.as_deref() {
                    Some("Strict") => Some(CookieSameSite::Strict),
                    Some("Lax") => Some(CookieSameSite::Lax),
                    Some("None") => Some(CookieSameSite::None),
                    _ => None,
                };

                let priority = match cookie.priority.as_str() {
                    "Low" => Some(CookiePriority::Low),
                    "Medium" => Some(CookiePriority::Medium),
                    "High" => Some(CookiePriority::High),
                    _ => None,
                };
                let expires = if cookie.session {
                    None
                } else {
                    Some(cookie.expires)
                };

                self.inner
                    .call_method(headless_chrome::protocol::cdp::Network::SetCookie {
                        name: cookie.name,
                        value: cookie.value,
                        url: session_url.clone(),
                        domain: Some(cookie.domain),
                        path: Some(cookie.path),
                        secure: Some(cookie.secure),
                        http_only: Some(cookie.http_only),
                        same_site,
                        expires,
                        priority,
                        same_party: None,
                        source_scheme: None,
                        source_port: None,
                        partition_key: None,
                    })
                    .context("Failed to set cookie")?;
            }
        }
        Ok(())
    }

    pub fn get_document_node(&self) -> Result<headless_chrome::protocol::cdp::DOM::Node> {
        // Enforce DOM domain enablement if not already?
        // Tab usually enables domains on demand or we might need to do it.
        // But let's try calling get_document.
        let root = self
            .inner
            .call_method(headless_chrome::protocol::cdp::DOM::GetDocument {
                depth: Some(1000), // Retrieve full depth? Or default? Default is usually deep?
                // spec says: "The maximum depth at which children should be retrieved, defaults to 1. Use -1 for the entire subtree".
                // We need full tree for SRE.
                // Using 1000 as a large enough depth since -1 (full) is not supported by headless_chrome u32 type.
                pierce: Some(false), // Keep the hot path on the main document tree unless explicitly needed.
            })?;
        Ok(root.root)
    }

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

    fn ensure_sre_event_bridge(&self) -> Result<u64> {
        let script = format!(
            r#"
(() => {{
    const bridgeKey = Symbol.for("{SRE_EVENT_BRIDGE_SYMBOL}");
    const existing = window[bridgeKey];
    const isValidExisting =
        existing &&
        typeof existing === "object" &&
        Number.isFinite(existing.version) &&
        Array.isArray(existing.waiters) &&
        Array.isArray(existing.dirtyPaths) &&
        existing.dirtyPathSet &&
        typeof existing.dirtyPathSet.clear === "function";

    if (isValidExisting) {{
        return Math.floor(existing.version);
    }}

    if (existing && existing.observer && typeof existing.observer.disconnect === "function") {{
        try {{
            existing.observer.disconnect();
        }} catch (_ignored) {{}}
    }}

    const state = {{
        version: 0,
        waiters: [],
        dirtyPaths: [],
        dirtyPathSet: new Set(),
    }};

    const toSemanticPath = (node) => {{
        if (!node) {{
            return null;
        }}

        if (node.nodeType === Node.DOCUMENT_NODE) {{
            return 'root/#document';
        }}

        let current = node;
        if (current.nodeType !== Node.ELEMENT_NODE) {{
            current = current.parentElement;
        }}
        if (!current) {{
            return null;
        }}

        const parts = [];
        while (current && current.nodeType === Node.ELEMENT_NODE) {{
            parts.push(current.tagName.toLowerCase());
            current = current.parentElement;
        }}

        parts.reverse();
        parts.unshift('#document');
        return `root/${{parts.join("/")}}`;
    }};

    const queuePath = (path) => {{
        if (typeof path !== "string" || path.length === 0) {{
            return;
        }}
        if (state.dirtyPathSet.has(path)) {{
            return;
        }}

        if (state.dirtyPaths.length >= 512) {{
            state.dirtyPaths = ['root/#document'];
            state.dirtyPathSet.clear();
            state.dirtyPathSet.add('root/#document');
            return;
        }}

        state.dirtyPathSet.add(path);
        state.dirtyPaths.push(path);
    }};

    const queueNodeAndParent = (node) => {{
        if (!node) {{
            return;
        }}
        queuePath(toSemanticPath(node));
        queuePath(toSemanticPath(node.parentElement));
    }};

    const notify = (mutations = []) => {{
        for (const mutation of mutations) {{
            const target = mutation.target;
            const elementTarget =
                target && target.nodeType === Node.ELEMENT_NODE
                    ? target
                    : target && target.parentElement
                        ? target.parentElement
                        : null;
            queueNodeAndParent(elementTarget);

            if (mutation.type === "childList") {{
                for (const added of mutation.addedNodes || []) {{
                    queueNodeAndParent(added);
                }}
                for (const removed of mutation.removedNodes || []) {{
                    queueNodeAndParent(mutation.target);
                }}
            }}
        }}

        state.version += 1;
        const waiters = state.waiters.splice(0, state.waiters.length);
        for (const waiter of waiters) {{
            try {{
                waiter(state.version);
            }} catch (_ignored) {{}}
        }}
    }};

    const target = document.documentElement || document;
    const observer = new MutationObserver((mutations) => notify(mutations));
    observer.observe(target, {{
        subtree: true,
        childList: true,
        attributes: true,
        characterData: true,
    }});

    state.observer = observer;
    window[bridgeKey] = state;
    return state.version;
}})()
"#
        );

        let value = self
            .evaluate_script_value(&script, false)
            .context("Failed to initialize SRE event bridge")?;
        value_to_u64(&value).context("Invalid SRE event bridge version")
    }

    fn wait_for_sre_event(&self, last_seen_version: u64, timeout: Duration) -> Result<u64> {
        let timeout_ms = duration_to_millis(timeout);
        let script = format!(
            r#"
(() => {{
    const bridgeKey = Symbol.for("{SRE_EVENT_BRIDGE_SYMBOL}");
    const state = window[bridgeKey];
    const isValidState =
        state &&
        typeof state === "object" &&
        Number.isFinite(state.version) &&
        Array.isArray(state.waiters);

    if (!isValidState) {{
        return Promise.resolve({last_seen_version});
    }}

    if (state.version > {last_seen_version}) {{
        return Promise.resolve(state.version);
    }}

    return new Promise((resolve) => {{
        let settled = false;
        const complete = (version) => {{
            if (settled) {{
                return;
            }}
            settled = true;
            clearTimeout(timer_id);
            const index = state.waiters.indexOf(waiter);
            if (index >= 0) {{
                state.waiters.splice(index, 1);
            }}
            resolve(version);
        }};

        const waiter = (version) => complete(version);
        const timer_id = setTimeout(() => complete(state.version), {timeout_ms});
        state.waiters.push(waiter);
    }});
}})()
"#
        );

        let value = self
            .evaluate_script_value(&script, true)
            .context("Failed while waiting for SRE event")?;
        value_to_u64(&value).context("Invalid SRE event version")
    }

    fn take_sre_dirty_paths(&self) -> Result<Vec<String>> {
        let script = format!(
            r#"
(() => {{
    const bridgeKey = Symbol.for("{SRE_EVENT_BRIDGE_SYMBOL}");
    const state = window[bridgeKey];
    const isValidState =
        state &&
        typeof state === "object" &&
        Array.isArray(state.dirtyPaths) &&
        state.dirtyPathSet &&
        typeof state.dirtyPathSet.clear === "function";

    if (!isValidState) {{
        return [];
    }}

    const paths = state.dirtyPaths.slice();
    state.dirtyPaths = [];
    state.dirtyPathSet.clear();
    return paths;
}})()
"#
        );

        let value = self
            .evaluate_script_value(&script, false)
            .context("Failed to fetch dirty semantic paths")?;
        value_to_string_vec(&value)
    }

    /// Explicitly request a visual capture with SoM marks.
    pub fn get_visual(&self) -> Result<VisualCapture> {
        self.capture_som(SomTrigger::GetVisual)
    }

    /// Returns the number of SoM captures generated for this page session.
    pub fn som_generation_count(&self) -> usize {
        self.som_pipeline
            .lock()
            .map(|state| state.generation_count)
            .unwrap_or_default()
    }

    /// Returns the latest SoM capture, if any.
    pub fn last_visual_capture(&self) -> Option<VisualCapture> {
        self.som_pipeline
            .lock()
            .ok()
            .and_then(|state| state.last_capture.clone())
    }

    /// Refresh the per-session stable_key index from the latest semantic capture.
    /// Returns the number of indexed nodes.
    pub fn refresh_stable_key_index(&self, profile: LoadProfile) -> Result<usize> {
        self.capture_state(profile)?;
        Ok(self
            .stable_key_index
            .lock()
            .map(|index| index.len())
            .unwrap_or_default())
    }

    /// Capture semantic state with the requested load profile.
    pub fn capture_semantic_state(&self, profile: LoadProfile) -> Result<SemanticState> {
        self.capture_state(profile)
    }

    /// Resolve the element bounding box `[x, y, width, height]` from a backend node id.
    pub fn get_element_bbox(&self, backend_node_id: i64) -> Result<Option<[f64; 4]>> {
        self.resolve_node_bbox(backend_node_id)
    }

    /// Resolve a backend node id from the per-session stable_key index.
    pub fn lookup_backend_node_id_by_stable_key(&self, stable_key: &str) -> Option<i64> {
        self.stable_key_index
            .lock()
            .ok()
            .and_then(|index| index.get(stable_key).map(|entry| entry.backend_node_id))
    }

    /// Resolve alias metadata from the per-session stable_key index.
    pub fn lookup_alias_by_stable_key(&self, stable_key: &str) -> Option<String> {
        self.stable_key_index
            .lock()
            .ok()
            .and_then(|index| index.get(stable_key).and_then(|entry| entry.alias.clone()))
    }

    /// Replace policy rules for this page session.
    pub fn set_policy_rules(&self, rules: Vec<PolicyRule>) -> Result<()> {
        let engine = PolicyEngine::try_new(rules)?;
        let mut engine_guard = self
            .policy_engine
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock policy engine"))?;
        *engine_guard = engine;
        drop(engine_guard);

        let mut approvals_guard = self
            .policy_approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock policy approval state"))?;
        approvals_guard.pending = None;
        approvals_guard.granted.clear();
        Ok(())
    }

    /// Returns the current pending human-approval request, if any.
    pub fn pending_policy_approval(&self) -> Option<PolicyApprovalRequest> {
        self.policy_approvals
            .lock()
            .ok()
            .and_then(|state| state.pending.clone())
    }

    /// Approve the latest pending policy request.
    pub fn approve_pending_policy_action(&self) -> Result<()> {
        let mut guard = self
            .policy_approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock policy approval state"))?;

        let Some(request) = guard.pending.take() else {
            anyhow::bail!("No pending policy approval request");
        };

        // Drop the lock before calling current_url() to avoid potential deadlock.
        drop(guard);

        let granted_navigation_epoch = self.navigation_epoch.load(Ordering::Relaxed);
        let granted_url = self.current_url().unwrap_or_default();
        let now_ms = epoch_millis();
        let (expires_at_epoch_ms, remaining_uses) = match request.scope {
            ApprovalScope::ActionOnly => (None, Some(1)),
            ApprovalScope::UntilNavigation => (None, None),
            ApprovalScope::Timeboxed { ms } => (Some(now_ms + u128::from(ms)), None),
        };

        let mut guard = self
            .policy_approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock policy approval state"))?;
        guard.granted.push(GrantedPolicyApproval {
            request,
            granted_navigation_epoch,
            granted_url,
            expires_at_epoch_ms,
            remaining_uses,
        });

        Ok(())
    }

    /// Reject the latest pending policy request.
    ///
    /// Unlike [`approve_pending_policy_action`], the request is discarded rather
    /// than recorded as granted — the originating action remains blocked. Used by
    /// HITL bridges (e.g. Slack/Teams reference implementations) to relay a human's
    /// "Reject" decision back into the session.
    ///
    /// [`approve_pending_policy_action`]: Self::approve_pending_policy_action
    pub fn reject_pending_policy_action(&self) -> Result<()> {
        let mut guard = self
            .policy_approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock policy approval state"))?;

        if guard.pending.take().is_none() {
            anyhow::bail!("No pending policy approval request");
        }
        drop(guard);

        self.audit_logger.log(AuditEvent::HitlEvent {
            event_type: "rejected".to_string(),
            reason: None,
            user_id: None,
            timestamp: epoch_millis_u64(),
        });

        Ok(())
    }

    /// Verify element text against an expected value.
    /// On mismatch, triggers a SoM capture to help disambiguate recovery.
    pub fn verify_text(&self, target_id: i64, expected_text: &str) -> Result<()> {
        self.audit_logger.log(AuditEvent::ToolCall {
            tool_name: "verify_text".to_string(),
            args: serde_json::json!({
                "target_id": target_id,
                "expected_text": expected_text
            }),
            timestamp: epoch_millis_u64(),
        });

        let actual_text = self.get_element_text(target_id)?;
        if normalize_text(&actual_text) == normalize_text(expected_text) {
            return Ok(());
        }

        self.trigger_som_capture_best_effort(SomTrigger::VerifyFailed);
        Err(VerifyError::ExpectationMismatch {
            target_id,
            expected: expected_text.to_string(),
            actual: actual_text,
        }
        .into())
    }

    /// Perform an action on a target element.
    /// Uses `target_id` (backend_node_id) preferentially.
    /// If `target_id` is invalid (stale), attempts fallback using `stable_key` by re-scanning the DOM.
    pub fn act(
        &self,
        target_id: Option<i64>,
        stable_key: Option<&str>,
        action: &str,
        value: Option<&str>,
    ) -> Result<()> {
        self.audit_logger.log(AuditEvent::ToolCall {
            tool_name: "act".to_string(),
            args: serde_json::json!({
                "target_id": target_id,
                "stable_key": stable_key,
                "action": action,
                "value": value
            }),
            timestamp: epoch_millis_u64(),
        });

        self.enforce_policy(target_id, stable_key, action)?;

        // First attempt: use target_id if available
        if let Some(bid) = target_id {
            match self.perform_action_by_id(bid, action, value) {
                Ok(_) => {
                    // Seed the DOMSignatureCache so future stale-key recovery can use it.
                    if let Some(key) = stable_key {
                        self.record_dom_signature_for_node_id(key, bid);
                    }
                    return Ok(());
                }
                Err(e) => {
                    // Only fallback if the error indicates a node issue (e.g. "Could not find node", "No node with given id")
                    // If it's a timeout or other error, we probably shouldn't blindly retry?
                    // The CDP error for invalid backend_node_id usually says "Could not find node with given id".
                    let node_error_markers = [
                        "could not find node",
                        "no node with given id",
                        "could not find object with given id",
                        "no object with given id",
                        "node does not exist",
                    ];
                    let is_node_error = error_chain_contains_any(&e, &node_error_markers);

                    if !is_node_error {
                        return Err(e);
                    }

                    if stable_key.is_none() {
                        self.record_action_log(
                            "error",
                            "verify_required",
                            action,
                            target_id,
                            stable_key,
                            "target_id lookup failed and no stable_key was provided",
                        );
                        self.trigger_som_capture_best_effort(SomTrigger::ActAmbiguous);
                        return Err(ActionError::VerifyRequired.into());
                    }
                    // Fallback proceed...
                }
            }
        }

        // Fallback: use stable_key
        if let Some(key) = stable_key {
            // Refresh semantic snapshot and stable-key index before lookup.
            self.capture_state(crate::sre::LoadProfile::Interactive)?;

            if let Some(new_id) = self.lookup_backend_node_id_by_stable_key(key) {
                self.record_action_log(
                    "warning",
                    "stable_key_fallback_recovered",
                    action,
                    target_id,
                    stable_key,
                    &format!("Action recovered via stable key: {key} -> new_id: {new_id}"),
                );
                // Record a fresh signature for the successfully located node.
                self.record_dom_signature_for_node_id(key, new_id);
                return self.perform_action_by_id(new_id, action, value);
            }

            // Both target_id and stable_key lookup failed →
            // attempt Self-Healing Context Recovery (PR-21).
            if let Some(recovered_id) = self.try_self_healing_recovery(key) {
                // Re-enforce policy against the recovered node so target-text /
                // surrounding-context rules are evaluated on the actual target.
                self.enforce_policy(Some(recovered_id), Some(key), action)?;
                return self.perform_action_by_id(recovered_id, action, value);
            }

            // Recovery failed → AskHumanRequired fallback.
            self.record_action_log(
                "error",
                "ask_human_required",
                action,
                target_id,
                stable_key,
                &format!(
                    "Self-healing recovery failed for stable_key={key}; human intervention required"
                ),
            );
            self.trigger_som_capture_best_effort(SomTrigger::ActAmbiguous);
            return Err(ActionError::AskHumanRequired {
                reason: format!(
                    "target_id and stable_key both failed for key={key}; \
                     self-healing found no confident match"
                ),
            }
            .into());
        }

        // If we reach here, we had no stable key or it failed lookup, and target_id failed or wasn't provided.
        // Actually, if stable_key was None, we would have returned early in the target_id block if target_id was Some.
        // If target_id was None AND stable_key was None, we should also error.

        self.record_action_log(
            "error",
            "verify_required",
            action,
            target_id,
            stable_key,
            "neither target_id nor stable_key resolved a target",
        );
        self.trigger_som_capture_best_effort(SomTrigger::ActAmbiguous);
        Err(ActionError::VerifyRequired.into())
    }

    // -------------------------------------------------------------------------
    // Self-Healing Context Recovery helpers (PR-21 / ISSUE-11)
    // -------------------------------------------------------------------------

    /// Attempt to recover a `backend_node_id` via `DOMSignatureCache` fuzzy
    /// matching when both `target_id` and `stable_key` index lookup have failed.
    ///
    /// On a confident match the cache is updated with the new `stable_key`→node
    /// mapping (learning), and the `backend_node_id` of the matched node is
    /// returned.  Returns `None` if no candidate exceeds the recovery threshold.
    fn try_self_healing_recovery(&self, stable_key: &str) -> Option<i64> {
        // Collect all interactive nodes from the current semantic snapshot.
        let state = self
            .semantic_capture_cache
            .lock()
            .ok()
            .and_then(|g| g.last_state.clone())?;

        let candidates = Self::collect_interactive_nodes(state.root());

        let matched = self
            .dom_signature_cache
            .find_best_match(stable_key, &candidates)?;

        let new_id = matched.backend_node_id;
        if new_id == 0 {
            return None;
        }

        // Learning: update cache with the newly discovered node.
        self.dom_signature_cache.record(stable_key, matched);

        // Redact label before logging to avoid PII leaking via action logs.
        let redacted_label = matched
            .label
            .as_deref()
            .map(|l| crate::privacy::global().redact_text(l));
        self.record_action_log(
            "warning",
            "self_healing_recovered",
            "self_healing",
            None,
            Some(stable_key),
            &format!(
                "Self-Healing: fuzzy match recovered stable_key={stable_key} \
                 to backend_node_id={new_id} (role={}, label={:?})",
                matched.role, redacted_label,
            ),
        );

        Some(new_id)
    }

    /// Record a `NodeSignature` in the `DOMSignatureCache` for the node
    /// identified by `backend_node_id` in the current stable_key index.
    fn record_dom_signature_for_node_id(&self, stable_key: &str, backend_node_id: i64) {
        // Walk the current semantic snapshot to find the node.
        let Some(state) = self
            .semantic_capture_cache
            .lock()
            .ok()
            .and_then(|g| g.last_state.clone())
        else {
            return;
        };

        fn find_node(root: &SemanticNode, id: i64) -> Option<&SemanticNode> {
            if root.backend_node_id == id {
                return Some(root);
            }
            root.children.iter().find_map(|c| find_node(c, id))
        }

        if let Some(node) = find_node(state.root(), backend_node_id) {
            self.dom_signature_cache.record(stable_key, node);
        }
    }

    /// Collect all interactive leaf nodes from a DOM tree (flat, depth-first).
    fn collect_interactive_nodes(root: &SemanticNode) -> Vec<SemanticNode> {
        let mut out = Vec::new();
        Self::collect_nodes_recursive(root, &mut out);
        out
    }

    fn collect_nodes_recursive(node: &SemanticNode, out: &mut Vec<SemanticNode>) {
        // Include nodes that have a non-zero backend_node_id (live DOM elements).
        if node.backend_node_id != 0 {
            out.push(node.clone());
        }
        for child in &node.children {
            Self::collect_nodes_recursive(child, out);
        }
    }

    // -------------------------------------------------------------------------

    fn record_action_log(
        &self,
        level: &str,
        code: &str,
        action: &str,
        target_id: Option<i64>,
        stable_key: Option<&str>,
        message: &str,
    ) {
        let entry = ActionLogEntry {
            level: level.to_string(),
            code: code.to_string(),
            action: action.to_string(),
            target_id,
            stable_key: stable_key.map(|key| key.to_string()),
            message: message.to_string(),
            timestamp: epoch_millis_u64(),
        };

        if let Ok(mut guard) = self.action_logs.lock() {
            guard.push_back(entry.clone());
            while guard.len() > ACTION_LOG_BUFFER_LIMIT {
                guard.pop_front();
            }
        } else {
            eprintln!("[ACTION][ERROR] Failed to lock structured action log buffer");
        }

        eprintln!(
            "[ACTION][{}] {}",
            level.to_uppercase(),
            serde_json::json!({
                "code": entry.code,
                "action": entry.action,
                "target_id": entry.target_id,
                "stable_key": entry.stable_key,
                "message": entry.message,
                "timestamp": entry.timestamp
            })
        );
    }

    fn enforce_policy(
        &self,
        target_id: Option<i64>,
        stable_key: Option<&str>,
        action: &str,
    ) -> Result<()> {
        let captured = self.capture_state(LoadProfile::Interactive)?;
        let target_node = resolve_policy_target_node(captured.root(), target_id, stable_key);

        let target_role = target_node.map(|node| node.role.clone());
        let target_text = target_node.and_then(policy_target_text);
        let surrounding_text = target_node.map(|node| policy_context_text(node, captured.root()));
        let target_signature = policy_target_signature(target_node, target_id, stable_key);
        let url = self.current_url()?;

        let decision = {
            let guard = self
                .policy_engine
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock policy engine"))?;
            guard.evaluate(&PolicyContext {
                url: url.clone(),
                action: action.to_string(),
                target_role,
                target_text,
                surrounding_text,
            })
        };

        self.audit_logger.log(AuditEvent::PolicyDecision {
            rule_id: decision
                .rule_id
                .clone()
                .unwrap_or_else(|| "unnamed-policy-rule".to_string()),
            action: action.to_string(),
            decision: match decision.action {
                PolicyAction::Allow => "allow".to_string(),
                PolicyAction::Block => "block".to_string(),
                PolicyAction::RequireHumanApproval => "require_human_approval".to_string(),
            },
            timestamp: epoch_millis_u64(),
        });

        match decision.action {
            PolicyAction::Allow => {
                // Built-in policy allows — now run plugin policy hooks.
                self.enforce_plugin_policy(action, &url)?;
                Ok(())
            }
            PolicyAction::Block => Err(ActionError::Blocked {
                rule_id: decision
                    .rule_id
                    .unwrap_or_else(|| "unnamed-policy-rule".to_string()),
            }
            .into()),
            PolicyAction::RequireHumanApproval => {
                let rule_id = decision
                    .rule_id
                    .unwrap_or_else(|| "unnamed-policy-rule".to_string());
                let scope = decision.scope.unwrap_or(ApprovalScope::ActionOnly);

                if self.consume_granted_policy_approval(
                    &rule_id,
                    scope,
                    action,
                    &target_signature,
                    &url,
                ) {
                    // Human approval was granted — still run plugin policy hooks so
                    // plugins have a chance to veto even pre-approved actions.
                    self.enforce_plugin_policy(action, &url)?;
                    return Ok(());
                }

                self.set_pending_policy_approval(PolicyApprovalRequest {
                    rule_id: rule_id.clone(),
                    scope,
                    action: action.to_string(),
                    target_signature,
                    outcome: decision.outcome.clone(),
                })?;

                self.audit_logger.log(AuditEvent::HitlEvent {
                    event_type: "request".to_string(),
                    reason: Some(format!("Requires human approval for rule {}", rule_id)),
                    user_id: None,
                    timestamp: epoch_millis_u64(),
                });

                Err(ActionError::HumanApprovalRequired {
                    rule_id,
                    scope,
                    outcome: decision.outcome,
                }
                .into())
            }
        }
    }

    /// Run all policy plugin hooks for the given action and URL.
    ///
    /// If any plugin blocks (or fails), the action is rejected (fail-closed).
    /// All plugin decisions are recorded in the audit log.
    fn enforce_plugin_policy(&self, action: &str, url: &str) -> Result<()> {
        let plugins = &self.plugin_hooks.policy_plugins;
        if plugins.is_empty() {
            return Ok(());
        }

        let intent_json = serde_json::json!({
            "action": action,
            "url": url,
        })
        .to_string();

        let (outcome, events) = run_policy_hooks(&intent_json, plugins.iter().map(|p| p.as_ref()));

        for event in events {
            self.audit_logger.log(event);
        }

        match outcome {
            PolicyHookOutcome::Allow => Ok(()),
            PolicyHookOutcome::Block { plugin_id, reason } => {
                let rule_id = format!("plugin:{plugin_id}");
                let reason_str = reason.as_deref().unwrap_or("blocked by plugin");
                eprintln!("[PLUGIN][POLICY] Action blocked by plugin {plugin_id}: {reason_str}");
                Err(ActionError::Blocked { rule_id }.into())
            }
        }
    }

    pub fn current_url(&self) -> Result<String> {
        let value = self.evaluate_script_value("window.location.href", false)?;
        value
            .as_str()
            .map(ToOwned::to_owned)
            .context("Failed to resolve current page URL for policy evaluation")
    }

    fn document_ready_state(&self) -> Result<String> {
        let value = self.evaluate_script_value("document.readyState", false)?;
        value
            .as_str()
            .map(ToOwned::to_owned)
            .context("Failed to resolve document.readyState while waiting for navigation")
    }

    fn wait_for_navigation_fallback(
        &self,
        requested_url: &str,
        previous_url: Option<&str>,
    ) -> Result<()> {
        let started = Instant::now();
        let (last_url, last_ready_state, last_dom_non_empty) = loop {
            let current_url = self.current_url().ok();
            let ready_state = self.document_ready_state().ok();
            let dom_non_empty = self
                .get_content()
                .map(|content| !content.trim().is_empty())
                .unwrap_or(false);

            if navigation_fallback_condition_met(
                requested_url,
                previous_url,
                current_url.as_deref(),
                ready_state.as_deref(),
                dom_non_empty,
            ) {
                return Ok(());
            }

            let remaining = remaining_timeout(started, NAVIGATION_FALLBACK_TIMEOUT);
            if remaining.is_zero() {
                break (current_url, ready_state, dom_non_empty);
            }
            thread::sleep(min(NAVIGATION_FALLBACK_POLL_INTERVAL, remaining));
        };

        anyhow::bail!(
            "Navigation fallback timed out after {}ms (requested_url={}, previous_url={:?}, current_url={:?}, ready_state={:?}, dom_non_empty={})",
            duration_to_millis(NAVIGATION_FALLBACK_TIMEOUT),
            requested_url,
            previous_url,
            last_url,
            last_ready_state,
            last_dom_non_empty
        );
    }

    fn set_pending_policy_approval(&self, request: PolicyApprovalRequest) -> Result<()> {
        let mut guard = self
            .policy_approvals
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock policy approval state"))?;
        guard.pending = Some(request);
        Ok(())
    }

    fn clear_pending_policy_approval(&self) {
        if let Ok(mut guard) = self.policy_approvals.lock() {
            guard.pending = None;
        }
    }

    fn consume_granted_policy_approval(
        &self,
        rule_id: &str,
        scope: ApprovalScope,
        action: &str,
        target_signature: &str,
        current_url: &str,
    ) -> bool {
        let now_ms = epoch_millis();
        let navigation_epoch = self.navigation_epoch.load(Ordering::Relaxed);

        let Ok(mut guard) = self.policy_approvals.lock() else {
            return false;
        };

        guard
            .granted
            .retain(|grant| is_grant_valid(grant, navigation_epoch, current_url, now_ms));

        let Some(index) = guard.granted.iter().position(|grant| {
            grant.request.rule_id == rule_id
                && grant.request.scope == scope
                && grant.request.action == action
                && grant.request.target_signature == target_signature
        }) else {
            return false;
        };

        match scope {
            ApprovalScope::ActionOnly => {
                let mut should_remove = true;
                if let Some(remaining) = guard.granted[index].remaining_uses.as_mut() {
                    if *remaining > 1 {
                        *remaining -= 1;
                        should_remove = false;
                    }
                }

                if should_remove {
                    guard.granted.remove(index);
                }
            }
            ApprovalScope::UntilNavigation | ApprovalScope::Timeboxed { .. } => {}
        }

        true
    }

    /// Wait until a semantic target reaches the requested state.
    pub fn wait_for_semantic(
        &self,
        target: SemanticTarget,
        desired_state: SemanticWaitState,
        timeout: Duration,
    ) -> Result<()> {
        self.wait_for_semantic_with_options(
            target,
            desired_state,
            timeout,
            SemanticWaitOptions::default(),
        )
    }

    /// Wait until a semantic target reaches the requested state with explicit options.
    pub fn wait_for_semantic_with_options(
        &self,
        target: SemanticTarget,
        desired_state: SemanticWaitState,
        timeout: Duration,
        options: SemanticWaitOptions,
    ) -> Result<()> {
        let mut subscriber = SreEventSubscriber::new(self, options.load_profile);
        let started = Instant::now();
        let transient_backoff = transient_error_backoff(options.poll_interval);

        loop {
            let remaining = remaining_timeout(started, timeout);
            if remaining.is_zero() {
                return Err(WaitError::Timeout {
                    operation: format!(
                        "semantic target {:?} to become {:?}",
                        target, desired_state
                    ),
                    timeout_ms: duration_to_millis(timeout),
                }
                .into());
            }

            match subscriber.wait_next_state_event(remaining) {
                Ok(Some(state)) => {
                    if target_matches_state(&state, &target, desired_state) {
                        return Ok(());
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    if !is_transient_capture_error(&err) {
                        return Err(err.context("Failed while waiting for semantic target state"));
                    }
                    sleep_transient_backoff(started, timeout, transient_backoff);
                }
            }
        }
    }

    /// Wait until the specified intent marker appears in semantic state updates.
    pub fn wait_for_intent(&self, intent: &str, timeout: Duration) -> Result<()> {
        self.wait_for_intent_with_options(intent, timeout, SemanticWaitOptions::default())
    }

    /// Wait until the specified intent marker appears with explicit options.
    pub fn wait_for_intent_with_options(
        &self,
        intent: &str,
        timeout: Duration,
        options: SemanticWaitOptions,
    ) -> Result<()> {
        let mut subscriber = SreEventSubscriber::new(self, options.load_profile);
        let started = Instant::now();
        let transient_backoff = transient_error_backoff(options.poll_interval);

        loop {
            let remaining = remaining_timeout(started, timeout);
            if remaining.is_zero() {
                return Err(WaitError::Timeout {
                    operation: format!("intent '{}'", intent),
                    timeout_ms: duration_to_millis(timeout),
                }
                .into());
            }

            match subscriber.wait_next_state_event(remaining) {
                Ok(Some(state)) => {
                    if state_contains_intent(state.root(), intent) {
                        return Ok(());
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    if !is_transient_capture_error(&err) {
                        return Err(err.context("Failed while waiting for intent"));
                    }
                    sleep_transient_backoff(started, timeout, transient_backoff);
                }
            }
        }
    }

    fn capture_state(&self, profile: LoadProfile) -> Result<SemanticState> {
        let cache = self.semantic_capture_cache_snapshot(profile)?;
        let raw_state = Arc::new(self.capture_state_with_refinement(profile, &[], None, None)?);
        self.record_state_update(cache.last_state.clone(), Arc::clone(&raw_state))?;

        // Apply state plugin hooks for external delivery (non-fatal on failure).
        // NOTE: The raw (unmodified) state is stored in the semantic capture cache so
        // that policy enforcement in `enforce_policy()` always operates on the
        // original SRE output, not on plugin-transformed data.  This preserves the
        // trust boundary: plugins may only transform state for external consumers;
        // they cannot influence internal policy decisions.
        let transformed_state = self.apply_state_hooks(Arc::clone(&raw_state))?;

        self.replace_semantic_capture_cache(profile, Arc::clone(&raw_state))?;
        Ok(transformed_state.as_ref().clone())
    }

    /// Serialize the `SemanticState` root, run all state plugins, then
    /// deserialize the result.  On any failure (serialization, plugin error,
    /// deserialization), log to audit and return the original state unchanged.
    fn apply_state_hooks(&self, state: Arc<SemanticState>) -> Result<Arc<SemanticState>> {
        let plugins = &self.plugin_hooks.state_plugins;
        if plugins.is_empty() {
            return Ok(state);
        }

        let state_json = match serde_json::to_string(state.root()) {
            Ok(json) => json,
            Err(err) => {
                eprintln!("[PLUGIN][WARN] Failed to serialize state for plugin hooks: {err}");
                return Ok(state);
            }
        };

        let (transformed_json, events) =
            run_state_hooks(&state_json, plugins.iter().map(|p| p.as_ref()));

        for event in events {
            self.audit_logger.log(event);
        }

        if transformed_json == state_json {
            // Nothing changed — reuse the existing Arc.
            return Ok(state);
        }

        // Attempt to deserialize the transformed JSON back into a SemanticNode.
        match serde_json::from_str::<SemanticNode>(&transformed_json) {
            Ok(new_root) => {
                let new_state = Arc::new(SemanticState::new(new_root, state.load_profile()));
                Ok(new_state)
            }
            Err(err) => {
                eprintln!(
                    "[PLUGIN][WARN] State plugin produced invalid JSON, reverting to original: {err}"
                );
                Ok(state)
            }
        }
    }

    /// Retrieve the actual browser viewport dimensions via JavaScript.
    ///
    /// Falls back to the default 800×600 constants if the CDP call fails, so that
    /// captures remain functional even when the browser context is unavailable.
    fn get_viewport_dimensions(&self) -> ViewportDimensions {
        let script = "JSON.stringify({width: window.innerWidth, height: window.innerHeight})";
        let Ok(value) = self.evaluate_script_value(script, false) else {
            return ViewportDimensions::default();
        };
        let Some(json_str) = value.as_str() else {
            return ViewportDimensions::default();
        };
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str) else {
            return ViewportDimensions::default();
        };
        let width = parsed
            .get("width")
            .and_then(|v| v.as_f64())
            .filter(|&w| w > 0.0);
        let height = parsed
            .get("height")
            .and_then(|v| v.as_f64())
            .filter(|&h| h > 0.0);
        match (width, height) {
            (Some(w), Some(h)) => ViewportDimensions {
                width: w,
                height: h,
            },
            _ => ViewportDimensions::default(),
        }
    }

    fn capture_state_with_refinement(
        &self,
        profile: LoadProfile,
        dirty_paths: &[String],
        cached_root: Option<&SemanticNode>,
        cached_paths: Option<&HashMap<String, Vec<usize>>>,
    ) -> Result<SemanticState> {
        let root = self.get_document_node()?;
        let viewport = self.get_viewport_dimensions();
        let normalized_dirty_paths = normalize_dirty_paths(dirty_paths);
        let sem_root = if let (Some(cached_root), Some(cached_paths)) = (cached_root, cached_paths)
        {
            if !normalized_dirty_paths.is_empty() && !cached_paths.is_empty() {
                normalize_dom_with_viewport_and_refinement(
                    profile,
                    &root,
                    viewport,
                    SubtreeRefinementConfig {
                        dirty_paths: &normalized_dirty_paths,
                        cached_paths,
                        cached_root,
                    },
                )?
            } else {
                normalize_dom_with_viewport(profile, &root, viewport)?
            }
        } else {
            normalize_dom_with_viewport(profile, &root, viewport)?
        };
        self.replace_stable_key_index(&sem_root);
        Ok(SemanticState::new(sem_root, profile))
    }

    fn replace_stable_key_index(&self, root: &SemanticNode) {
        if let Ok(mut index) = self.stable_key_index.lock() {
            index.clear();
            collect_stable_key_entries(root, &mut index);
        }
    }

    fn clear_stable_key_index(&self) {
        if let Ok(mut index) = self.stable_key_index.lock() {
            index.clear();
        }
    }

    fn semantic_capture_cache_snapshot(
        &self,
        profile: LoadProfile,
    ) -> Result<SemanticCaptureCache> {
        let cache = self
            .semantic_capture_cache
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock semantic capture cache"))?
            .clone();

        if cache.profile == Some(profile) {
            Ok(cache)
        } else {
            Ok(SemanticCaptureCache::default())
        }
    }

    fn replace_semantic_capture_cache(
        &self,
        profile: LoadProfile,
        state: Arc<SemanticState>,
    ) -> Result<()> {
        let mut cache = self
            .semantic_capture_cache
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock semantic capture cache"))?;
        cache.profile = Some(profile);
        cache.last_state = Some(state);
        Ok(())
    }

    fn clear_semantic_capture_cache(&self) {
        if let Ok(mut cache) = self.semantic_capture_cache.lock() {
            *cache = SemanticCaptureCache::default();
        }
    }

    fn record_state_update(
        &self,
        previous: Option<Arc<SemanticState>>,
        current: Arc<SemanticState>,
    ) -> Result<()> {
        self.audit_logger.log_state_update(previous, current);
        Ok(())
    }

    fn capture_som(&self, trigger: SomTrigger) -> Result<VisualCapture> {
        let root = self.get_document_node()?;
        let viewport = self.get_viewport_dimensions();
        let semantic_root = normalize_dom_with_viewport(LoadProfile::Visual, &root, viewport)?;

        let mut marks = Vec::new();
        self.collect_som_marks(&semantic_root, &mut marks);

        let image_png = self
            .inner
            .capture_screenshot(
                headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
                None,
                None,
                true,
            )
            .context("Failed to capture SoM screenshot")?;

        let capture = VisualCapture {
            trigger,
            marks,
            image_png,
        };

        self.audit_logger.log(AuditEvent::VisualCapture {
            trigger: match capture.trigger {
                SomTrigger::GetVisual => "get_visual".to_string(),
                SomTrigger::ActAmbiguous => "act_ambiguous".to_string(),
                SomTrigger::VerifyFailed => "verify_failed".to_string(),
            },
            marks_count: capture.marks.len(),
            timestamp: epoch_millis_u64(),
        });

        self.store_som_capture(capture.clone());
        Ok(capture)
    }

    fn trigger_som_capture_best_effort(&self, trigger: SomTrigger) {
        if let Err(err) = self.capture_som(trigger) {
            eprintln!("[WARN] SoM capture trigger failed ({trigger:?}): {err:#}");
        }
    }

    fn store_som_capture(&self, capture: VisualCapture) {
        if let Ok(mut state) = self.som_pipeline.lock() {
            state.generation_count += 1;
            state.last_capture = Some(capture);
        }
    }

    fn collect_som_marks(&self, node: &SemanticNode, out: &mut Vec<SomMark>) {
        if node.backend_node_id > 0 && node.stable_key.is_some() {
            if let Ok(Some(bbox)) = self.resolve_node_bbox(node.backend_node_id) {
                out.push(SomMark {
                    id: node.backend_node_id,
                    stable_key: node.stable_key.clone(),
                    bbox,
                });
            }
        }

        for child in &node.children {
            self.collect_som_marks(child, out);
        }
    }

    fn resolve_node_bbox(&self, backend_node_id: i64) -> Result<Option<[f64; 4]>> {
        let node_id_u32 =
            u32::try_from(backend_node_id).context("Invalid backend_node_id: must fit in u32")?;

        let model = self
            .inner
            .call_method(headless_chrome::protocol::cdp::DOM::GetBoxModel {
                node_id: None,
                backend_node_id: Some(node_id_u32),
                object_id: None,
            })
            .context("Failed to get box model")?
            .model;

        Ok(quad_to_bbox(&model.content))
    }

    fn get_element_text(&self, backend_node_id: i64) -> Result<String> {
        use headless_chrome::protocol::cdp::Runtime::CallFunctionOn;

        let object_id = self.resolve_node_object_id(backend_node_id)?;
        let result = self
            .inner
            .call_method(CallFunctionOn {
                object_id: Some(object_id),
                function_declaration:
                    "function() { return (this.innerText || this.textContent || '').trim(); }"
                        .to_string(),
                arguments: None,
                silent: Some(true),
                return_by_value: Some(true),
                generate_preview: Some(false),
                user_gesture: Some(false),
                await_promise: Some(false),
                execution_context_id: None,
                object_group: None,
                throw_on_side_effect: None,
                unique_context_id: None,
                serialization_options: None,
            })
            .context("Failed to extract element text")?;

        let value = result
            .result
            .value
            .unwrap_or_else(|| serde_json::Value::String(String::new()));

        if let Some(text) = value.as_str() {
            Ok(text.to_string())
        } else {
            Ok(value.to_string())
        }
    }

    fn resolve_node_object_id(&self, backend_node_id: i64) -> Result<String> {
        use headless_chrome::protocol::cdp::DOM::ResolveNode;

        let node_id_u32 =
            u32::try_from(backend_node_id).context("Invalid backend_node_id: must fit in u32")?;

        let remote_object = self
            .inner
            .call_method(ResolveNode {
                node_id: None,
                backend_node_id: Some(node_id_u32),
                object_group: None,
                execution_context_id: None,
            })
            .context("Failed to resolve node")?
            .object;

        remote_object
            .object_id
            .context("Failed to resolve node to object")
    }

    fn perform_action_by_id(
        &self,
        backend_node_id: i64,
        action: &str,
        value: Option<&str>,
    ) -> Result<()> {
        // Resolve backend_node_id to RemoteObject
        use headless_chrome::protocol::cdp::Runtime::CallFunctionOn;
        let object_id = self.resolve_node_object_id(backend_node_id)?;

        match action {
            "click" => {
                self.inner.call_method(CallFunctionOn {
                    object_id: Some(object_id),
                    function_declaration: "function() { this.click(); }".to_string(),
                    arguments: None,
                    silent: Some(true),
                    return_by_value: Some(false),
                    generate_preview: Some(false),
                    user_gesture: Some(true),
                    await_promise: Some(false),
                    execution_context_id: None,
                    object_group: None,
                    throw_on_side_effect: None,
                    unique_context_id: None,
                    serialization_options: None,
                })?;
            }
            "type" => {
                let text = value.context("Value is required for type action")?;
                // Focus then type
                self.inner.call_method(CallFunctionOn {
                    object_id: Some(object_id.clone()),
                    function_declaration: "function() { this.focus(); }".to_string(),
                    arguments: None,
                    silent: Some(true),
                    return_by_value: Some(false),
                    generate_preview: Some(false),
                    user_gesture: Some(true),
                    await_promise: Some(false),
                    execution_context_id: None,
                    object_group: None,
                    throw_on_side_effect: None,
                    unique_context_id: None,
                    serialization_options: None,
                })?;
                self.inner.type_str(text)?;
            }
            _ => anyhow::bail!("Unsupported action: {}", action),
        }
        Ok(())
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

fn target_matches_state(
    state: &SemanticState,
    target: &SemanticTarget,
    desired_state: SemanticWaitState,
) -> bool {
    let resolved = match target {
        SemanticTarget::Id(id) => find_node_by_id(state.root(), *id),
        SemanticTarget::StableKey(key) => find_node_by_key(state.root(), key),
        SemanticTarget::IdWithStableKey { id, stable_key } => find_node_by_id(state.root(), *id)
            .or_else(|| find_node_by_key(state.root(), stable_key)),
    };

    resolved.is_some_and(|node| node_matches_state(node, desired_state))
}

fn find_node_by_id(node: &SemanticNode, target_id: i64) -> Option<&SemanticNode> {
    if node.backend_node_id == target_id {
        return Some(node);
    }

    for child in &node.children {
        if let Some(found) = find_node_by_id(child, target_id) {
            return Some(found);
        }
    }

    None
}

fn find_node_by_key<'a>(node: &'a SemanticNode, target_key: &str) -> Option<&'a SemanticNode> {
    if let Some(key) = &node.stable_key {
        if key == target_key {
            return Some(node);
        }
    }

    for child in &node.children {
        if let Some(found) = find_node_by_key(child, target_key) {
            return Some(found);
        }
    }

    None
}

fn normalize_dirty_paths(raw_paths: &[String]) -> HashSet<String> {
    raw_paths
        .iter()
        .map(|path| path.trim())
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn build_semantic_path_index(root: &SemanticNode) -> HashMap<String, Vec<usize>> {
    let mut counts = HashMap::new();
    count_semantic_paths(root, "root", &mut counts);

    let mut index = HashMap::new();
    let mut current_child_path = Vec::new();
    collect_unique_semantic_paths(root, "root", &counts, &mut current_child_path, &mut index);
    index
}

fn count_semantic_paths(
    node: &SemanticNode,
    parent_path: &str,
    counts: &mut HashMap<String, usize>,
) {
    let current_path = semantic_path(parent_path, &node.role);
    *counts.entry(current_path.clone()).or_insert(0) += 1;

    for child in &node.children {
        count_semantic_paths(child, &current_path, counts);
    }
}

fn collect_unique_semantic_paths(
    node: &SemanticNode,
    parent_path: &str,
    counts: &HashMap<String, usize>,
    current_child_path: &mut Vec<usize>,
    index: &mut HashMap<String, Vec<usize>>,
) {
    let current_path = semantic_path(parent_path, &node.role);
    if counts.get(&current_path).copied() == Some(1) {
        index.insert(current_path.clone(), current_child_path.clone());
    }

    for (child_index, child) in node.children.iter().enumerate() {
        current_child_path.push(child_index);
        collect_unique_semantic_paths(child, &current_path, counts, current_child_path, index);
        current_child_path.pop();
    }
}

fn semantic_path(parent_path: &str, role: &str) -> String {
    format!("{}/{}", parent_path, role.to_lowercase())
}

fn collect_stable_key_entries(node: &SemanticNode, out: &mut HashMap<String, StableKeyIndexEntry>) {
    if let Some(key) = &node.stable_key {
        if node.backend_node_id > 0 {
            out.insert(
                key.clone(),
                StableKeyIndexEntry {
                    backend_node_id: node.backend_node_id,
                    alias: node.alias.clone(),
                },
            );
        }
    }

    for child in &node.children {
        collect_stable_key_entries(child, out);
    }
}

fn quad_to_bbox(quad: &[f64]) -> Option<[f64; 4]> {
    if quad.len() < 8 {
        return None;
    }

    let xs = [quad[0], quad[2], quad[4], quad[6]];
    let ys = [quad[1], quad[3], quad[5], quad[7]];

    let min_x = xs.iter().copied().fold(f64::INFINITY, f64::min);
    let max_x = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let min_y = ys.iter().copied().fold(f64::INFINITY, f64::min);
    let max_y = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max);

    if !min_x.is_finite() || !max_x.is_finite() || !min_y.is_finite() || !max_y.is_finite() {
        return None;
    }

    Some([
        min_x,
        min_y,
        (max_x - min_x).max(0.0),
        (max_y - min_y).max(0.0),
    ])
}

fn normalize_text(input: &str) -> String {
    input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn node_matches_state(node: &SemanticNode, desired_state: SemanticWaitState) -> bool {
    match desired_state {
        SemanticWaitState::Enabled => node_is_enabled(node),
    }
}

fn node_is_enabled(node: &SemanticNode) -> bool {
    !node
        .attributes
        .as_ref()
        .is_some_and(|attrs| attrs.contains_key("disabled"))
}

fn state_contains_intent(root: &SemanticNode, intent: &str) -> bool {
    let normalized_intent = intent.trim().to_lowercase();
    node_contains_intent(root, &normalized_intent)
}

fn node_contains_intent(node: &SemanticNode, intent: &str) -> bool {
    if node
        .label
        .as_deref()
        .is_some_and(|label| label.trim().to_lowercase() == intent)
    {
        return true;
    }

    if let Some(attrs) = &node.attributes {
        for (key, value) in attrs {
            let key_normalized = key.to_lowercase();
            let value_normalized = value.to_lowercase();
            if (key_normalized == "data-intent" || key_normalized == "intent")
                && value_normalized == intent
            {
                return true;
            }
        }
    }

    for child in &node.children {
        if node_contains_intent(child, intent) {
            return true;
        }
    }

    false
}

fn error_chain_contains_any(err: &anyhow::Error, markers: &[&str]) -> bool {
    err.chain().any(|source| {
        let message = source.to_string().to_lowercase();
        markers.iter().any(|marker| message.contains(marker))
    })
}

fn is_transient_capture_error(err: &anyhow::Error) -> bool {
    let transient_markers = [
        "could not find node",
        "no node with given id",
        "execution context was destroyed",
        "cannot find context with specified id",
        "navigation",
    ];

    error_chain_contains_any(err, &transient_markers)
}

fn remaining_timeout(started: Instant, timeout: Duration) -> Duration {
    timeout.saturating_sub(started.elapsed())
}

fn normalized_poll_interval(poll_interval: Duration) -> Duration {
    if poll_interval.is_zero() {
        DEFAULT_WAIT_POLL_INTERVAL
    } else {
        poll_interval
    }
}

fn transient_error_backoff(poll_interval: Duration) -> Duration {
    min(
        normalized_poll_interval(poll_interval),
        MAX_TRANSIENT_ERROR_BACKOFF,
    )
}

fn navigation_fallback_condition_met(
    requested_url: &str,
    previous_url: Option<&str>,
    current_url: Option<&str>,
    ready_state: Option<&str>,
    dom_non_empty: bool,
) -> bool {
    let reached_requested_url = current_url.is_some_and(|url| url == requested_url);
    let moved_to_new_url = match (previous_url, current_url) {
        (_, None) => false,
        (Some(previous), Some(current)) => current != previous,
        (None, Some(_)) => true,
    };
    let dom_ready = matches!(ready_state, Some("interactive" | "complete"));

    (reached_requested_url || moved_to_new_url) && (dom_ready || dom_non_empty)
}

fn sleep_transient_backoff(started: Instant, timeout: Duration, poll_interval: Duration) {
    let remaining = remaining_timeout(started, timeout);
    if remaining.is_zero() {
        return;
    }

    thread::sleep(min(poll_interval, remaining));
}

fn value_to_u64(value: &serde_json::Value) -> Result<u64> {
    if let Some(v) = value.as_u64() {
        return Ok(v);
    }

    if let Some(v) = value.as_i64() {
        return u64::try_from(v).context("Expected non-negative integer value");
    }

    anyhow::bail!("Expected integer value, received: {value}");
}

fn value_to_string_vec(value: &serde_json::Value) -> Result<Vec<String>> {
    let entries = value
        .as_array()
        .context("Expected array value for dirty semantic paths")?;

    Ok(entries
        .iter()
        .filter_map(|entry| entry.as_str())
        .map(ToOwned::to_owned)
        .collect())
}

fn duration_to_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// Epoch milliseconds truncated to u64 for audit event timestamps.
/// Safe until ~year 584 million. Debug-asserts no truncation occurred.
fn epoch_millis_u64() -> u64 {
    let ms = epoch_millis();
    debug_assert!(ms <= u128::from(u64::MAX), "epoch_millis overflowed u64");
    ms as u64
}

#[cfg(test)]
mod tests {
    use super::*;

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
