use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticState {
    pub page_instance_id: String,
    pub state_hash: String,
    pub timestamp: u64,
    pub root: SemanticNode,
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
}

impl SemanticState {
    pub fn new(root: SemanticNode) -> Self {
        let mut state = Self {
            page_instance_id: uuid::Uuid::new_v4().to_string(),
            state_hash: String::new(), // Placeholder
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            root,
        };
        state.state_hash = state.calculate_hash();
        state
    }

    fn calculate_hash(&self) -> String {
        // We exclude state_hash itself (obviously) and page_instance_id/timestamp
        // because we want hash to represent the semantic CONTENT.
        // Actually, SPEC says state_hash.
        // Ideally we hash the ROOT content.
        // If two pages have same content but different timestamp, should hash be same?
        // SPEC SRE-01: "state_hash を含む". "出力: Semantic State JSON".
        // "Equivalent Input -> Equivalent Hash".
        // So Timestamp and ID should NOT be part of the hash input.
        // Only Root.

        // Serialize root to canonical JSON (BTreeMap ensures deterministic map order)
        let json_content = serde_json::to_string(&self.root).unwrap_or_default();

        let mut hasher = Sha256::new();
        hasher.update(json_content);
        let result = hasher.finalize();
        hex::encode(result)
    }
}
