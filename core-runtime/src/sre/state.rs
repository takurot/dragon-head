use super::profile::LoadProfile;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Semantic State: the deterministic, AI-optimized representation of a web page.
/// SPEC SRE-01: output includes page_instance_id, state_hash, timestamp, load_profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticState {
    page_instance_id: String,
    state_hash: String,
    timestamp: u64,
    load_profile: LoadProfile,
    root: SemanticNode,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SemanticNode {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<SemanticNode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<BTreeMap<String, String>>,
    // New fields for ACT-01
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stable_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ambiguous: Option<bool>,
}

impl SemanticState {
    /// Create a new SemanticState. The state_hash is computed from the root content only,
    /// ensuring that identical semantic content always produces the same hash.
    pub fn new(root: SemanticNode, load_profile: LoadProfile) -> Self {
        let state_hash = Self::compute_hash(&root);
        Self {
            page_instance_id: uuid::Uuid::new_v4().to_string(),
            state_hash,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            load_profile,
            root,
        }
    }

    /// Accessor for state_hash (read-only).
    pub fn state_hash(&self) -> &str {
        &self.state_hash
    }

    /// Accessor for root (read-only).
    pub fn root(&self) -> &SemanticNode {
        &self.root
    }

    /// Accessor for load_profile (read-only).
    pub fn load_profile(&self) -> LoadProfile {
        self.load_profile
    }

    /// Accessor for page_instance_id (read-only).
    pub fn page_instance_id(&self) -> &str {
        &self.page_instance_id
    }

    /// Compute deterministic SHA-256 hash of the semantic root content.
    /// Excludes page_instance_id, timestamp, and load_profile — only the
    /// semantic tree contributes to the hash.
    fn compute_hash(root: &SemanticNode) -> String {
        // BTreeMap ensures deterministic map iteration order for serialization
        let json_content = serde_json::to_string(root).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(json_content);
        hex::encode(hasher.finalize())
    }
}
