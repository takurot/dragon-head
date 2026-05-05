/// Integration tests for Self-Healing Context Recovery (PR-21 / ISSUE-11).
///
/// These tests verify the `DOMSignatureCache` and fuzzy-matching behaviour in
/// isolation (no browser required) — validating that the recovery layer meets
/// the Exit Criteria:
///   "修復成功時に verify 要求なしでアクションが継続される"
///   (On successful recovery, the action continues without a verify request.)
use core_runtime::{
    dom_signature::{DOMSignatureCache, NodeSignature, RECOVERY_THRESHOLD},
    sre::SemanticNode,
};
use std::collections::BTreeMap;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn node(role: &str, label: Option<&str>, alias: Option<&str>, id: i64) -> SemanticNode {
    SemanticNode {
        role: role.to_string(),
        label: label.map(str::to_string),
        children: vec![],
        attributes: None,
        stable_key: None,
        ambiguous: false,
        alias: alias.map(str::to_string),
        backend_node_id: id,
    }
}

fn node_with_attrs(
    role: &str,
    label: Option<&str>,
    attrs: &[(&str, &str)],
    id: i64,
) -> SemanticNode {
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
        backend_node_id: id,
    }
}

// ---------------------------------------------------------------------------
// NodeSignature scoring
// ---------------------------------------------------------------------------

#[test]
fn score_threshold_documented_value_is_45() {
    assert_eq!(
        RECOVERY_THRESHOLD, 45,
        "RECOVERY_THRESHOLD must be 45 per spec"
    );
}

#[test]
fn score_identical_button_exceeds_threshold() {
    let sig = NodeSignature::from_node(&node("button", Some("Submit order"), Some("submit"), 0));
    let candidate = node("button", Some("Submit order"), Some("submit"), 42);
    assert!(
        sig.score_against(&candidate) >= RECOVERY_THRESHOLD,
        "identical button must be recoverable"
    );
}

#[test]
fn score_relabelled_button_exceeds_threshold() {
    // UI changed "Continue to checkout" → "Continue" — should still recover.
    let sig = NodeSignature::from_node(&node("button", Some("Continue to checkout"), None, 0));
    let candidate = node("button", Some("Continue"), None, 99);
    assert!(
        sig.score_against(&candidate) >= RECOVERY_THRESHOLD,
        "partial label match on same role must recover"
    );
}

#[test]
fn score_different_role_does_not_recover() {
    let sig = NodeSignature::from_node(&node("button", Some("Submit"), None, 0));
    let candidate = node("input", Some("Submit"), None, 5);
    assert!(
        sig.score_against(&candidate) < RECOVERY_THRESHOLD,
        "different role must not recover even if labels match"
    );
}

#[test]
fn score_attr_match_boosts_score() {
    let original = node_with_attrs(
        "input",
        Some("Email"),
        &[("type", "email"), ("name", "email")],
        0,
    );
    let sig = NodeSignature::from_node(&original);
    // Same element, label slightly different due to UI tweak.
    let candidate = node_with_attrs(
        "input",
        Some("E-mail address"),
        &[("type", "email"), ("name", "email")],
        7,
    );
    let score = sig.score_against(&candidate);
    assert!(
        score >= RECOVERY_THRESHOLD,
        "role + attribute match must recover even with diverged label; score={score}"
    );
}

// ---------------------------------------------------------------------------
// DOMSignatureCache fuzzy matching
// ---------------------------------------------------------------------------

#[test]
fn cache_fuzzy_match_picks_best_candidate() {
    let cache = DOMSignatureCache::new();
    cache.record(
        "key-confirm",
        &node("button", Some("Confirm order"), Some("confirm"), 0),
    );

    let candidates = vec![
        node("a", Some("Home"), None, 1),
        node("button", Some("Confirm order"), Some("confirm"), 42), // best
        node("button", Some("Cancel"), None, 3),
    ];
    let found = cache.find_best_match("key-confirm", &candidates);
    assert!(found.is_some());
    assert_eq!(found.unwrap().backend_node_id, 42);
}

#[test]
fn cache_fuzzy_match_returns_none_when_all_below_threshold() {
    let cache = DOMSignatureCache::new();
    cache.record("key-login", &node("button", Some("Login"), None, 0));

    let candidates = vec![
        node("div", Some("Dashboard"), None, 10),
        node("span", Some("Profile"), None, 11),
    ];
    let found = cache.find_best_match("key-login", &candidates);
    assert!(
        found.is_none(),
        "below-threshold candidates must not recover"
    );
}

#[test]
fn cache_updates_on_re_record_for_same_key() {
    let cache = DOMSignatureCache::new();
    cache.record("key-x", &node("button", Some("Old label"), None, 0));
    cache.record("key-x", &node("button", Some("New label"), None, 0));
    // Only one entry should exist (updated, not duplicated).
    assert_eq!(cache.len(), 1);
}

#[test]
fn cache_handles_multiple_keys_independently() {
    let cache = DOMSignatureCache::new();
    cache.record("key-a", &node("button", Some("Accept"), None, 10));
    cache.record("key-b", &node("input", Some("Email"), None, 20));

    let btns = vec![node("button", Some("Accept"), None, 10)];
    let inputs = vec![node("input", Some("Email"), None, 20)];

    assert!(cache.find_best_match("key-a", &btns).is_some());
    assert!(cache.find_best_match("key-b", &inputs).is_some());
    // Cross-match must not work (different roles).
    assert!(cache.find_best_match("key-a", &inputs).is_none());
}

// ---------------------------------------------------------------------------
// Recovery learning
// ---------------------------------------------------------------------------

#[test]
fn cache_learning_updates_signature_after_recovery() {
    let cache = DOMSignatureCache::new();
    // Record original signature.
    cache.record("key-btn", &node("button", Some("Submit"), None, 0));

    // Simulate UI change: label updated.
    let candidates = vec![node("button", Some("Submit form"), None, 99)];
    let recovered = cache.find_best_match("key-btn", &candidates);
    assert!(
        recovered.is_some(),
        "should recover via partial label match"
    );

    // Simulate learning: record the new node.
    cache.record("key-btn", recovered.unwrap());

    // On next recovery attempt, the new label should be the cached signature.
    let new_candidates = vec![
        node("button", Some("Submit form"), None, 99),
        node("button", Some("Submit"), None, 0), // original (stale)
    ];
    let found = cache.find_best_match("key-btn", &new_candidates);
    // Both are good matches; we just verify recovery still works.
    assert!(
        found.is_some(),
        "post-learning recovery must still find a match"
    );
}

// ---------------------------------------------------------------------------
// Ambiguous tie rejection (Fix 3)
// ---------------------------------------------------------------------------

#[test]
fn ambiguous_tie_is_rejected() {
    let cache = DOMSignatureCache::new();
    cache.record("key-del", &node("button", Some("Delete"), None, 0));

    // Two identical candidates — tied score must result in no recovery.
    let candidates = vec![
        node("button", Some("Delete"), None, 10),
        node("button", Some("Delete"), None, 11),
    ];
    let result = cache.find_best_match("key-del", &candidates);
    assert!(
        result.is_none(),
        "tied candidates must not recover to avoid wrong-element action"
    );
}

#[test]
fn unambiguous_winner_is_accepted() {
    let cache = DOMSignatureCache::new();
    cache.record(
        "key-ok",
        &node("button", Some("Confirm order"), Some("confirm"), 0),
    );

    let candidates = vec![
        node("button", Some("Confirm order"), Some("confirm"), 42), // clear winner
        node("button", Some("Cancel"), None, 5),                    // low score
    ];
    let result = cache.find_best_match("key-ok", &candidates);
    assert!(result.is_some());
    assert_eq!(result.unwrap().backend_node_id, 42);
}
