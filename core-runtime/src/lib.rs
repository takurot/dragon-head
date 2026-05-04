pub mod audit;
pub mod audit_sink;
pub mod browser;
pub mod chrome_detection;
pub mod plugin_hooks;
pub mod policy;
pub mod privacy;
pub mod session_vault;
pub mod sre;

// Re-export SRE types used by examples and downstream crates.
pub use sre::{
    FastSemanticState, FullSemanticState, LayeredSemanticState, LoadProfile, SemanticDelta,
    SemanticNode, SemanticState, StateUpdate,
};

pub use browser::{
    ActionLogEntry, BrowserClient, PageSession, SemanticTarget, SemanticWaitOptions,
    SemanticWaitState, SomMark, SomTrigger, VisualCapture,
};
pub mod error;
pub use chrome_detection::chrome_available;
pub use error::{ActionError, VerifyError, WaitError};
pub use plugin_hooks::PluginHookConfig;
pub use policy::{
    ApprovalScope, PolicyAction, PolicyContext, PolicyDecision, PolicyEngine, PolicyRule,
};
pub use session_vault::{KmsAdapter, LocalSessionVault, SessionData, SessionVault, SoftwareKms};
