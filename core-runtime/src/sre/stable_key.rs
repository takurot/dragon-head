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
    /// Returns (stable_key, is_duplicate).
    pub fn generate_key(
        &mut self,
        role: &str,
        label: Option<&str>,
        // dom_signature related args:
        parent_path: &str,
        // We include parent_path (e.g. parent's role or simplified path) to differentiate
        // identical elements in different containers.
    ) -> String {
        let label_part = label.unwrap_or("").trim().to_lowercase();

        // Base content for hashing: role + label + parent_path
        // This is a simplified version of "dom_signature".
        // Using `|` as separator.
        let content = format!("{}|{}|{}", role, label_part, parent_path);

        // Compute base hash (SHA-256)
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let result = hasher.finalize();
        let base_hash = hex::encode(result);

        // Collision handling: check occurrence count within this page generation session
        let count = self.counts.entry(base_hash.clone()).or_insert(0);
        *count += 1;

        if *count == 1 {
            // First occurrence: use base hash
            base_hash
        } else {
            // Collision: append index (e.g. hash_2, hash_3)
            // Note: SPEC says `ambiguous: true` should follow, but StableKey logic typically just returns the key string.
            // The caller (normalization) should handle the ambiguous flag if needed,
            // or we encode it in the key? The key itself should be unique.
            format!("{}_{}", base_hash, count)
        }
    }
}
