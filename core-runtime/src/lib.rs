// ISSUE-254: `core-runtime` is reachable from the stdio `dragon-head-mcp` binary, which uses
// stdout exclusively for line-delimited JSON-RPC framing. Any `println!`/`print!` here would
// write directly into that stream and corrupt it for the client.
#![warn(clippy::print_stdout)]

// ISSUE-206: this crate's diagnostics (audit mirroring, policy/plugin-hook decisions, capture
// failures, etc.) are emitted exclusively through `tracing::info!`/`warn!`/`error!`, not
// `eprintln!` — a `tracing` event is a silent no-op unless the consuming binary installs a
// subscriber. `dragon-head-mcp`, `dragon-head-bench`, and `dragon-head-hitl-bridge` each install
// one (stderr-only for the first two, which must keep stdout clean for other reasons); any other
// consumer of this crate — a different binary, a test, or an external application — must install
// its own subscriber (e.g. `tracing_subscriber::fmt::init()`) to observe these events at all.

pub mod audit;
pub mod audit_replay;
pub mod audit_sink;
pub mod browser;
pub mod chrome_detection;
pub mod dom_signature;
pub mod plugin_hooks;
pub mod policy;
pub mod privacy;
pub mod prompt_injection;
pub mod session_vault;
pub mod speculative;
pub mod sre;

// Re-export SRE types used by examples and downstream crates.
pub use sre::{
    DeltaPolicy, FastSemanticState, FullSemanticState, LayeredSemanticState, LoadProfile,
    SemanticDelta, SemanticNode, SemanticState, StateUpdate,
};

pub use browser::{
    is_browser_disconnected, is_transport_error, validate_public_navigation_url,
    validate_public_navigation_url_with, ActionLogEntry, BrowserClient, NavigationNetworkPolicy,
    NavigationValidationError, PageSession, SemanticTarget, SemanticWaitOptions, SemanticWaitState,
    SomMark, SomTrigger, ValidatedNavigationUrl, VisualCapture, HEALTH_CHECK_TIMEOUT,
    MAX_PUBLIC_NAVIGATION_URL_BYTES, STABLE_KEY_SHORT_LEN,
};
pub mod error;
pub use audit_sink::DurabilityMode;
pub use chrome_detection::chrome_available;
pub use error::{ActionError, SessionError, VerifyError, WaitError};
pub use plugin_hooks::PluginHookConfig;
pub use policy::{
    ApprovalScope, OutcomeProjection, OutcomeProjectorConfig, PolicyAction, PolicyContext,
    PolicyDecision, PolicyEngine, PolicyRule, RiskLevel,
};
pub use prompt_injection::{
    PromptInjectionMode, PromptInjectionSanitizer, PromptInjectionSanitizerConfig,
};
pub use session_vault::{
    AtomicKmsRotation, KmsAdapter, LocalSessionVault, SessionData, SessionVault, SoftwareKms,
};
