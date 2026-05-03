pub mod audit;
pub mod browser;
pub mod chrome_detection;
pub mod plugin_hooks;
pub mod policy;
pub mod session_vault;
pub mod sre;

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
