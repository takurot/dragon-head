// ISSUE-254: `dragon-head-mcp` uses stdout exclusively for line-delimited JSON-RPC framing. Any
// `println!`/`print!` outside an explicitly allowed human-facing CLI path (`--doctor`; see
// `doctor::print_report`) would write directly into that stream and corrupt it for the client.
// `eprintln!` (stderr) remains fine and is used extensively for startup/diagnostic logging.
#![warn(clippy::print_stdout)]

pub mod config;
pub mod doctor;
pub mod dto;
pub mod hitl;
pub mod metering;
pub mod plugins;
pub(crate) mod protocol;
pub mod self_test;

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use core_runtime::{
    speculative::{ActionSignature, SpeculativeEngine, SpeculativePrediction, StateDelta},
    sre::{LoadProfile, SemanticNode},
    validate_public_navigation_url, ActionError, ApprovalScope, BrowserClient, DeltaPolicy,
    NavigationNetworkPolicy, PageSession, PolicyRule, PromptInjectionMode,
    PromptInjectionSanitizer, PromptInjectionSanitizerConfig, SemanticState, SemanticTarget,
    SemanticWaitState, SessionError, StateUpdate, VerifyError, STABLE_KEY_SHORT_LEN,
};
// Internal-only metering types (pub(crate) in metering.rs)
use metering::{PlanFeature, UsageMeters};
use plugin_host::{ExtractionRule, SchemaRegistry};
use protocol::{
    negotiate_protocol_version, sanitize_log_field, serialize_response, JsonRpcRequest,
    JsonRpcResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use skills_engine::{
    parse_skill_definition, ActStep, ExtractStep, LocateStep, OperationOutcome, SkillDefinition,
    SkillEngine, SkillRunStatus, SkillRuntime, VerifyStep, WaitStep,
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

const MAX_VISUAL_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

// Re-export public types at the crate level to preserve the existing public API.
mod backend;
mod backend_tools;
mod convert;
mod extract;
mod schemas;
mod server;
mod skill_runtime;
mod speculative_state;
#[cfg(test)]
mod tests;
mod tool_args;

use convert::*;
use extract::*;
use hitl_bridge::gateway::SessionProvider;
pub use schemas::semantic_state_json_schema;
use schemas::*;
#[cfg(test)]
use server::*;
use skill_runtime::*;
use speculative_state::*;
use tool_args::*;

pub use dto::{ExternalInteractiveElement, ExternalSemanticState, StateMetadata};
pub use metering::{
    estimate_usage_cost, AuditRetentionSnapshot, PlanTier, SkillUsageDelta, StateGenerationUsage,
    UsageCostBreakdown, UsageReport,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

pub trait McpBackend {
    fn navigate(&mut self, arguments: Value) -> Result<Value>;
    fn get_state(&mut self, arguments: Value) -> Result<Value>;
    fn act(&mut self, arguments: Value) -> Result<Value>;
    fn verify(&mut self, arguments: Value) -> Result<Value>;
    fn get_visual(&mut self, arguments: Value) -> Result<Value>;
    fn ask_human(&mut self, arguments: Value) -> Result<Value>;
    fn run_skill(&mut self, arguments: Value) -> Result<Value>;
    fn extract(&mut self, arguments: Value) -> Result<Value>;
    /// Consume image bytes produced by the most recent successful `get_visual` call.
    /// Backends without binary visual output retain the metadata-only behavior.
    fn take_visual_image(&mut self) -> Option<Vec<u8>> {
        None
    }
    fn audit_retention_snapshot(&self) -> Option<AuditRetentionSnapshot> {
        None
    }
    /// Consume and return metered operations accumulated by the last `run_skill` call.
    /// The default implementation returns an empty delta (no metering).
    fn take_skill_usage_delta(&mut self) -> SkillUsageDelta {
        SkillUsageDelta::default()
    }

    /// Called when [`McpServer::call_tool`] detects that the underlying Chrome
    /// process disconnected (ISSUE-149). On success, returns the updated
    /// restart count; on failure, returns a human-readable reason.
    ///
    /// The default implementation reports that restart is not supported,
    /// preserving backward compatibility for backends without a managed
    /// browser process.
    fn handle_browser_disconnect(&mut self) -> std::result::Result<u64, String> {
        Err("browser restart not supported".to_string())
    }

    /// Confirms whether a suspected browser disconnect (detected via
    /// [`core_runtime::is_browser_disconnected`]) is a genuine Chrome
    /// crash/exit rather than a false positive caused by a client-side
    /// request timeout on a slow-but-alive operation (ISSUE-261's
    /// `get_visual`/`get_state` transport-busy scenario).
    ///
    /// Called by [`McpServer::call_tool_output`] before committing to
    /// [`handle_browser_disconnect`](Self::handle_browser_disconnect): when
    /// this returns `false`, the original error is returned to the caller
    /// unchanged and no restart is performed.
    ///
    /// The default implementation reports "confirmed disconnected"
    /// (`true`), preserving the prior always-restart-on-marker-match
    /// behavior for backends without a lightweight liveness probe
    /// available (e.g. test doubles).
    fn confirm_browser_disconnected(&self) -> bool {
        true
    }

    /// Total number of automatic browser restarts performed so far.
    fn browser_restart_count(&self) -> u64 {
        0
    }

    /// Best-effort probe of whether the underlying Chrome process is still
    /// running (ISSUE-260). Returns `Some(true)`/`Some(false)` when the
    /// backend can determine liveness (e.g. a managed `BrowserClient` with a
    /// known PID); `None` when it cannot (e.g. no managed process, or the
    /// platform lacks a liveness probe).
    ///
    /// [`McpServer::call_tool_output`] uses this to avoid tearing down the
    /// whole session when a disconnect-shaped error turns out to be a
    /// single-tab CDP hiccup rather than an actual Chrome crash. The default
    /// implementation reports unknown, preserving the pre-existing
    /// restart-on-any-disconnect-marker behavior for backends that don't
    /// manage a browser process (e.g. tests).
    fn is_chrome_process_alive(&self) -> Option<bool> {
        None
    }

    /// Cumulative `(hits, misses)` of the speculative state generation
    /// pipeline (Spec §3.5 / ISSUE-147). A hit is a `get_state` call served
    /// from a verified pre-generated snapshot; a miss is a `get_state` call
    /// where a prediction was attempted but did not yield a usable snapshot,
    /// falling back to a full capture.
    ///
    /// The default implementation reports no speculative activity,
    /// preserving backward compatibility for backends that don't wire the
    /// speculative engine.
    fn speculative_usage(&self) -> (u64, u64) {
        (0, 0)
    }
}

/// Maximum number of consecutive disconnect-shaped errors that will be
/// swallowed (error propagated, no restart) while the Chrome process is
/// confirmed alive (ISSUE-260), before escalating to a full restart anyway.
/// Bounds the risk of a permanently wedged single-tab CDP session (e.g. a
/// stale PID reused by an unrelated process, or a tab whose CDP session is
/// truly and irrecoverably dead) never triggering recovery.
const MAX_ALIVE_SKIPS_BEFORE_RESTART: u32 = 2;

pub struct McpServer<B> {
    backend: B,
    plan_tier: PlanTier,
    usage_meters: UsageMeters,
    /// Consecutive disconnect-shaped errors skipped (not restarted) because
    /// the backend confirmed Chrome was still alive (ISSUE-260). Reset to 0
    /// on any successful tool call.
    consecutive_alive_skips: u32,
}

/// Maximum number of automatic browser restarts allowed within
/// [`RESTART_RATE_LIMIT_WINDOW`] before [`CoreRuntimeBackend::handle_browser_disconnect`]
/// gives up and reports a persistent failure (ISSUE-149 restart-storm guard).
const RESTART_RATE_LIMIT_MAX: usize = 3;

/// Sliding window over which [`RESTART_RATE_LIMIT_MAX`] restarts are counted.
const RESTART_RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

pub struct CoreRuntimeBackend {
    page: Arc<PageSession>,
    /// Mirror of `page` that an embedded HITL bridge (ISSUE-302) re-reads on every poll via
    /// [`CoreRuntimeBackend::page_provider`], so it observes and resolves the same pending policy
    /// approvals as this backend and follows the swap made by browser-restart recovery
    /// (ISSUE-149, ISSUE-336). Must be updated wherever `page` is replaced.
    shared_page: Arc<std::sync::RwLock<Arc<PageSession>>>,
    state_cache: Option<ExternalSemanticState>,
    previous_semantic_state: Option<SemanticState>,
    skill_engine: SkillEngine,
    skills: HashMap<String, SkillDefinition>,
    last_skill_delta: SkillUsageDelta,
    pending_visual_image: Option<Vec<u8>>,
    schema_registry: SchemaRegistry,
    /// Prompt-injection sanitizer applied before any SemanticState is exposed to the LLM.
    injection_sanitizer: PromptInjectionSanitizer,
    /// Managed Chrome process, used to relaunch on disconnect (ISSUE-149).
    /// `None` for backends constructed via [`CoreRuntimeBackend::new`] (e.g. tests),
    /// which cannot recover from a browser disconnect.
    client: Option<BrowserClient>,
    /// Policy rules to reapply to the page created by a browser restart.
    policy_rules: Vec<PolicyRule>,
    /// Whether public navigation may target private and otherwise non-global networks.
    /// Defaults to false and is set from the resolved startup configuration.
    navigation_allow_private_network: bool,
    /// Total number of successful automatic browser restarts.
    browser_restarts: u64,
    /// Timestamps of recent restart attempts, used for rate limiting.
    restart_history: VecDeque<Instant>,
    /// Speculative state generation engine (Spec §3.5 / ISSUE-147).
    speculative: SpeculativeEngine,
    /// The action that led to `previous_semantic_state`, used as the
    /// `last_action` input to [`SpeculativeEngine::predict`]/`pre_generate`.
    last_action: Option<ActionSignature>,
    /// The action executed by the most recent successful `act` call, not yet
    /// confirmed against a subsequent `get_state`.
    pending_action: Option<ActionSignature>,
    /// Count of `get_state` calls served from a verified speculative
    /// pre-generation.
    speculative_hits: u64,
    /// Count of `get_state` calls where a prediction was attempted but did
    /// not yield a usable snapshot.
    speculative_misses: u64,
    /// The prediction behind the most recently served speculative hit, kept
    /// so it can be verified against the next real DOM capture (Spec §3.5 /
    /// ISSUE-147 review: a served hit is otherwise never checked against the
    /// live page).
    last_served_prediction: Option<SpeculativePrediction>,
    /// Set when a second successful `act` occurs before the next
    /// `get_state` (Spec §3.5 / ISSUE-147 review). No single
    /// `ActionSignature` describes the combined effect of a chained action
    /// sequence, so `record_speculative_observation` must neither record a
    /// transition for it nor verify `last_served_prediction` against the
    /// resulting capture (which reflects more than one action past the
    /// prediction). Cleared once that capture has been processed.
    action_chain_broken: bool,
    /// Whether `previous_semantic_state` reflects a real DOM capture that has
    /// been observed by the speculative engine, as opposed to an unverified
    /// speculative snapshot served by the `Hit` branch (Spec §3.5 /
    /// ISSUE-147 round-9 review). `record_speculative_observation` must not
    /// record a `previous -> current` transition from an unverified
    /// speculative snapshot: if that snapshot was stale, the recorded edge
    /// would train the engine on a transition that does not exist in the
    /// live page's transition graph.
    previous_state_verified: bool,
    /// The `state_hash` of `previous_semantic_state` at the time
    /// `last_served_prediction` was generated, i.e. the `from_state_hash`
    /// of the `(from_state_hash, predicted_action) -> predicted_state_hash`
    /// transition that prediction represents (Spec §3.5 / ISSUE-147
    /// round-10 review). Needed to correct that transition in the
    /// transition model if `last_served_prediction` turns out to have been
    /// stale, since by the time it is verified `previous_semantic_state`
    /// has already been overwritten with the (unverified) speculative
    /// snapshot the prediction described.
    last_served_prediction_source_hash: Option<String>,
}
