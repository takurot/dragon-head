/// Self-Healing Context Recovery Layer (PR-21 / ISSUE-11).
///
/// When both `target_id` and `stable_key` lookup fail during `act()`, this
/// module attempts to recover by fuzzy-matching the cached DOM signature of
/// the intended element against the current live DOM tree.
///
/// Recovery flow (ACT-04 step 3):
/// 1. Cache records `NodeSignature` on every successful operation.
/// 2. When stable_key is missing/stale, `DOMSignatureCache::find_best_match`
///    walks all interactive DOM nodes and returns the closest match.
/// 3. On success: action proceeds, cache is updated with the new stable_key.
/// 4. On failure: `ActionError::AskHumanRequired` is returned.
use crate::sre::state::SemanticNode;
use std::{
    collections::{BTreeMap, HashMap},
    sync::Mutex,
};

// ---------------------------------------------------------------------------
// NodeSignature
// ---------------------------------------------------------------------------

/// Structural fingerprint of a successfully interacted-with DOM node.
///
/// Captured at operation time and compared against live nodes during recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeSignature {
    pub role: String,
    pub label: Option<String>,
    pub alias: Option<String>,
    /// Attribute subset relevant to identity: `type`, `name`, `id`, `placeholder`.
    pub attributes: BTreeMap<String, String>,
}

impl NodeSignature {
    /// Build a signature from a `SemanticNode`.
    pub fn from_node(node: &SemanticNode) -> Self {
        const IDENTITY_ATTRS: &[&str] = &["type", "name", "id", "placeholder", "aria-label"];
        let attributes = node
            .attributes
            .as_ref()
            .map(|a| {
                a.iter()
                    .filter(|(k, _)| IDENTITY_ATTRS.contains(&k.as_str()))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect()
            })
            .unwrap_or_default();

        Self {
            role: node.role.clone(),
            label: node.label.clone(),
            alias: node.alias.clone(),
            attributes,
        }
    }

    /// Compute a similarity score against a live `SemanticNode`.
    ///
    /// Returns a value in `0..=100`.  Scores ≥ `RECOVERY_THRESHOLD` are
    /// considered a confident match.
    pub fn score_against(&self, candidate: &SemanticNode) -> u32 {
        let mut score = 0u32;

        // Role match (exact): primary discriminator.
        if self.role == candidate.role {
            score += 35;
        }

        // Label similarity.
        score += label_similarity(self.label.as_deref(), candidate.label.as_deref());

        // Alias match.
        if let (Some(a), Some(b)) = (&self.alias, &candidate.alias) {
            if a == b {
                score += 20;
            }
        }

        // Attribute overlap (up to 10 points per matching key-value pair).
        if let Some(candidate_attrs) = &candidate.attributes {
            let matching = self
                .attributes
                .iter()
                .filter(|(k, v)| candidate_attrs.get(*k).is_some_and(|cv| cv == *v))
                .count();
            score += (matching as u32).min(3) * 10;
        }

        score.min(100)
    }
}

/// Minimum similarity score required to accept a fuzzy-match recovery.
///
/// Calibrated so that role-match + any meaningful label word overlap clears
/// the bar (role=35 + partial label≥10 = 45), while role-only or
/// label-only matches do not (max 35 without label).
pub const RECOVERY_THRESHOLD: u32 = 45;

/// Compute a word-overlap similarity score between two optional label strings.
///
/// Returns a value in `0..=30`.
fn label_similarity(a: Option<&str>, b: Option<&str>) -> u32 {
    let (a, b) = match (a, b) {
        (Some(a), Some(b)) => (a, b),
        _ => return 0,
    };

    if a == b {
        return 30;
    }

    // Word-overlap Jaccard: |A ∩ B| / |A ∪ B| × 30.
    let words_a: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let words_b: std::collections::HashSet<&str> = b.split_whitespace().collect();
    if words_a.is_empty() && words_b.is_empty() {
        return 0;
    }
    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();
    if union == 0 {
        return 0;
    }
    ((intersection as f32 / union as f32) * 30.0) as u32
}

// ---------------------------------------------------------------------------
// DOMSignatureCache
// ---------------------------------------------------------------------------

/// Thread-safe cache mapping `stable_key` → `NodeSignature`.
///
/// Records signatures on successful `act()` calls and queries them during
/// Self-Healing recovery.
#[derive(Debug, Default)]
pub struct DOMSignatureCache {
    entries: Mutex<HashMap<String, NodeSignature>>,
}

impl DOMSignatureCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record or update the signature for `stable_key`.
    pub fn record(&self, stable_key: &str, node: &SemanticNode) {
        if let Ok(mut guard) = self.entries.lock() {
            guard.insert(stable_key.to_string(), NodeSignature::from_node(node));
        }
    }

    /// Walk `nodes` (flat slice of `SemanticNode`) and return the node with
    /// the highest signature score against the cached entry for `stable_key`.
    ///
    /// Returns `None` if no entry exists for `stable_key` or no candidate
    /// exceeds `RECOVERY_THRESHOLD`.
    pub fn find_best_match<'a>(
        &self,
        stable_key: &str,
        nodes: &'a [SemanticNode],
    ) -> Option<&'a SemanticNode> {
        let guard = self.entries.lock().ok()?;
        let sig = guard.get(stable_key)?;

        nodes
            .iter()
            .map(|n| (n, sig.score_against(n)))
            .filter(|(_, score)| *score >= RECOVERY_THRESHOLD)
            .max_by_key(|(_, score)| *score)
            .map(|(n, _)| n)
    }

    /// Return the number of cached signatures.
    pub fn len(&self) -> usize {
        self.entries.lock().map(|g| g.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sre::state::SemanticNode;
    use std::collections::BTreeMap;

    fn node(role: &str, label: Option<&str>, alias: Option<&str>) -> SemanticNode {
        SemanticNode {
            role: role.to_string(),
            label: label.map(str::to_string),
            children: vec![],
            attributes: None,
            stable_key: None,
            ambiguous: false,
            alias: alias.map(str::to_string),
            backend_node_id: 0,
        }
    }

    fn node_with_attrs(role: &str, label: Option<&str>, attrs: &[(&str, &str)]) -> SemanticNode {
        let mut map = BTreeMap::new();
        for (k, v) in attrs {
            map.insert(k.to_string(), v.to_string());
        }
        SemanticNode {
            role: role.to_string(),
            label: label.map(str::to_string),
            children: vec![],
            attributes: Some(map),
            stable_key: None,
            ambiguous: false,
            alias: None,
            backend_node_id: 0,
        }
    }

    // -- NodeSignature --------------------------------------------------------

    #[test]
    fn node_signature_from_node_captures_identity_attrs() {
        let n = node_with_attrs(
            "input",
            Some("Email"),
            &[("type", "email"), ("name", "email")],
        );
        let sig = NodeSignature::from_node(&n);
        assert_eq!(sig.role, "input");
        assert_eq!(sig.label.as_deref(), Some("Email"));
        assert_eq!(
            sig.attributes.get("type").map(String::as_str),
            Some("email")
        );
    }

    #[test]
    fn node_signature_ignores_non_identity_attrs() {
        let n = node_with_attrs(
            "input",
            None,
            &[("data-analytics", "btn1"), ("style", "red")],
        );
        let sig = NodeSignature::from_node(&n);
        assert!(
            sig.attributes.is_empty(),
            "non-identity attrs must be dropped"
        );
    }

    // -- score_against --------------------------------------------------------

    #[test]
    fn score_exact_match_is_high() {
        let cached = NodeSignature {
            role: "button".to_string(),
            label: Some("Submit".to_string()),
            alias: Some("submit_btn".to_string()),
            attributes: BTreeMap::new(),
        };
        let candidate = node("button", Some("Submit"), Some("submit_btn"));
        assert!(
            cached.score_against(&candidate) >= RECOVERY_THRESHOLD,
            "exact role+label+alias must exceed threshold"
        );
    }

    #[test]
    fn score_role_mismatch_is_low() {
        let cached = NodeSignature {
            role: "button".to_string(),
            label: Some("Submit".to_string()),
            alias: None,
            attributes: BTreeMap::new(),
        };
        let candidate = node("input", Some("Submit"), None);
        let score = cached.score_against(&candidate);
        assert!(
            score < RECOVERY_THRESHOLD,
            "role mismatch should not recover; score={score}"
        );
    }

    #[test]
    fn score_partial_label_overlap_contributes() {
        let cached = NodeSignature {
            role: "button".to_string(),
            label: Some("Continue to checkout".to_string()),
            alias: None,
            attributes: BTreeMap::new(),
        };
        let candidate = node("button", Some("Continue".to_string().as_str()), None);
        let score = cached.score_against(&candidate);
        // role (35) + partial label — should recover
        assert!(
            score >= RECOVERY_THRESHOLD,
            "partial label overlap should recover; score={score}"
        );
    }

    #[test]
    fn score_no_overlap_is_zero() {
        let cached = NodeSignature {
            role: "a".to_string(),
            label: Some("Home".to_string()),
            alias: None,
            attributes: BTreeMap::new(),
        };
        let candidate = node("input", Some("Password".to_string().as_str()), None);
        let score = cached.score_against(&candidate);
        assert!(
            score < RECOVERY_THRESHOLD,
            "no overlap must not recover; score={score}"
        );
    }

    // -- DOMSignatureCache ----------------------------------------------------

    #[test]
    fn cache_records_and_retrieves_best_match() {
        let cache = DOMSignatureCache::new();
        let original = node("button", Some("Confirm order"), Some("confirm"));
        cache.record("key-abc", &original);

        // Slightly changed label — simulates a minor UI tweak.
        let candidates = vec![
            node("input", Some("Name"), None),
            node("button", Some("Confirm order"), Some("confirm")), // exact
            node("a", Some("Cancel"), None),
        ];

        let found = cache.find_best_match("key-abc", &candidates);
        assert!(found.is_some(), "must find the matching button");
        assert_eq!(found.unwrap().role, "button");
    }

    #[test]
    fn cache_returns_none_for_unknown_key() {
        let cache = DOMSignatureCache::new();
        let candidates = vec![node("button", Some("Click me"), None)];
        let found = cache.find_best_match("unknown-key", &candidates);
        assert!(found.is_none(), "unknown key must return None");
    }

    #[test]
    fn cache_returns_none_when_no_candidate_exceeds_threshold() {
        let cache = DOMSignatureCache::new();
        let original = node("button", Some("Confirm order"), Some("confirm"));
        cache.record("key-xyz", &original);

        let candidates = vec![
            node("input", Some("Totally different element"), None),
            node("a", Some("Another link"), None),
        ];
        let found = cache.find_best_match("key-xyz", &candidates);
        assert!(found.is_none(), "no confident match must return None");
    }

    #[test]
    fn cache_updates_signature_on_re_record() {
        let cache = DOMSignatureCache::new();
        cache.record("key-1", &node("button", Some("Old label"), None));
        cache.record("key-1", &node("button", Some("New label"), None));
        assert_eq!(cache.len(), 1, "same key must not create duplicate entries");
    }

    #[test]
    fn cache_is_thread_safe_concurrent_records() {
        use std::sync::Arc;
        use std::thread;

        let cache = Arc::new(DOMSignatureCache::new());
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let c = Arc::clone(&cache);
                thread::spawn(move || {
                    c.record(&format!("key-{i}"), &node("button", Some("Click"), None));
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(cache.len(), 10);
    }
}
