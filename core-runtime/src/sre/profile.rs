use serde::{Deserialize, Serialize};

/// Load profile controls what resources the SRE includes during normalization.
/// SPEC SRE-01: minimal / visual / interactive
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LoadProfile {
    #[default]
    Minimal,
    Visual,
    Interactive,
}
