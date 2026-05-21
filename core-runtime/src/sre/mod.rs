pub mod normalization;
pub mod pipeline;
pub mod profile;
pub mod stable_key;
pub mod state;

pub use crate::prompt_injection::PromptInjectionMode;
pub use normalization::{
    normalize_dom, normalize_dom_with_refinement, normalize_dom_with_viewport,
    normalize_dom_with_viewport_and_refinement, SubtreeRefinementConfig, ViewportDimensions,
};
pub use pipeline::{
    AsyncPipeline, AsyncPipelineConfig, AsyncPipelineMetrics, AuditEvent, AuditEventKind,
    LayeredStateHandle, PipelineReceiveError, PipelineSubmitError, QueueKind, SreStage,
};
pub use profile::LoadProfile;
pub use stable_key::StableKeyGenerator;
pub use state::{
    DeltaPolicy, FastSemanticState, FullSemanticState, LayeredSemanticState, SemanticDelta,
    SemanticNode, SemanticState, StateGenerationPhase, StateUpdate,
};
