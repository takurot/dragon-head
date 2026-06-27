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
    is_browser_disconnected, ActionLogEntry, BrowserClient, PageSession, SemanticTarget,
    SemanticWaitOptions, SemanticWaitState, SomMark, SomTrigger, VisualCapture,
    STABLE_KEY_SHORT_LEN,
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
pub use session_vault::{KmsAdapter, LocalSessionVault, SessionData, SessionVault, SoftwareKms};
