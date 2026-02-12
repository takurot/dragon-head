use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Helper struct to generate unique stable keys for Semantic Nodes.
/// Uses content-based hashing and index tracking for collision detection.
pub struct StableKeyGenerator {
    // Map of (base_hash) -> count
    counts: HashMap<String, usize>,
}

impl StableKeyGenerator {
    pub fn new() -> Self {
        Self {
            counts: HashMap::new(),
        }
    }

    /// Generates a stable key for a node based on its semantic content and context.
    pub fn generate_key(&mut self, role: &str, label: Option<&str>, parent_path: &str) -> String {
        let label_part = label.unwrap_or("").trim().to_lowercase();

        let content = format!("{}|{}|{}", role, label_part, parent_path);

        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let result = hasher.finalize();
        let base_hash = hex::encode(result);

        let count = self.counts.entry(base_hash.clone()).or_insert(0);
        *count += 1;

        if *count == 1 {
            base_hash
        } else {
            format!("{}_{}", base_hash, count)
        }
    }
}

impl Default for StableKeyGenerator {
    fn default() -> Self {
        Self::new()
    }
}
