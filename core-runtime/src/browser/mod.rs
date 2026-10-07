mod act;
mod audit;
#[cfg(test)]
mod browser_tests;
mod capture;
mod client;
mod helpers;
mod navigation;
mod navigation_policy;
mod page_navigation;
mod policy;
#[cfg(test)]
mod tests;
mod vault;
mod visual;
#[cfg(test)]
use client::confirm_alive_with;
pub use client::{is_browser_disconnected, is_transport_error};
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
use navigation_policy::*;

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

/// Number of hex characters exposed in the external (LLM-facing) stable_key.
/// 16 hex chars = 64 bits of entropy; negligible collision probability at page scale.
/// Re-exported from `core-runtime` so downstream crates share a single source of truth.
pub const STABLE_KEY_SHORT_LEN: usize = 16;
