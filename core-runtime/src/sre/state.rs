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

    /// Stable key for element identity (SHA-256 hex)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stable_key: Option<String>,

    /// True if the stable key collided and was resolved by index
    #[serde(default)]
    pub ambiguous: bool,

    /// Human-readable alias (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,

    // New field for ACT-04
    #[serde(default, rename = "id")]
    pub backend_node_id: i64,
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
        // Clone and strip volatile fields (backend_node_id) to ensure deterministic hash.
        // backend_node_id changes across sessions, but state_hash must be stable for same content.
        let mut clean_root = root.clone();
        Self::strip_volatile_fields(&mut clean_root);

        // BTreeMap ensures deterministic map iteration order for serialization
        let json_content = serde_json::to_string(&clean_root).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(json_content);
        hex::encode(hasher.finalize())
    }

    fn strip_volatile_fields(node: &mut SemanticNode) {
        node.backend_node_id = 0;
        for child in &mut node.children {
            Self::strip_volatile_fields(child);
        }
    }
}
