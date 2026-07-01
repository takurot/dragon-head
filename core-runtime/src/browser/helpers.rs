use anyhow::{Context, Result};
use std::{
    cmp::min,
    collections::{HashMap, HashSet},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::sre::{SemanticNode, SemanticState};

use super::{
    SemanticTarget, SemanticWaitState, StableKeyIndexEntry, DEFAULT_WAIT_POLL_INTERVAL,
    MAX_TRANSIENT_ERROR_BACKOFF, STABLE_KEY_SHORT_LEN,
};

pub(super) fn target_matches_state(
    state: &SemanticState,
    target: &SemanticTarget,
    desired_state: SemanticWaitState,
) -> bool {
    let resolved = match target {
        SemanticTarget::Id(id) => find_node_by_id(state.root(), *id),
        SemanticTarget::StableKey(key) => find_node_by_key(state.root(), key),
        SemanticTarget::IdWithStableKey { id, stable_key } => find_node_by_id(state.root(), *id)
            .or_else(|| find_node_by_key(state.root(), stable_key)),
    };

    resolved.is_some_and(|node| node_matches_state(node, desired_state))
}

pub(super) fn find_node_by_id(node: &SemanticNode, target_id: i64) -> Option<&SemanticNode> {
    if node.backend_node_id == target_id {
        return Some(node);
    }

    for child in &node.children {
        if let Some(found) = find_node_by_id(child, target_id) {
            return Some(found);
        }
    }

    None
}

pub(super) fn find_node_by_key<'a>(
    node: &'a SemanticNode,
    target_key: &str,
) -> Option<&'a SemanticNode> {
    if let Some(key) = &node.stable_key {
        // Reject empty keys before any comparison: cmp_len=0 would make every
        // node match, bypassing policy target resolution.
        if !target_key.is_empty() && !key.is_empty() {
            // Compare using the first STABLE_KEY_SHORT_LEN chars so that a caller
            // supplying a shortened key (as returned by get_state) matches the full
            // 64-char SHA-256 stored on SemanticNode.  Both sides are clamped to the
            // shorter length, so the comparison is always symmetric.
            let cmp_len = STABLE_KEY_SHORT_LEN.min(key.len()).min(target_key.len());
            if key[..cmp_len] == target_key[..cmp_len] {
                return Some(node);
            }
        }
    }

    for child in &node.children {
        if let Some(found) = find_node_by_key(child, target_key) {
            return Some(found);
        }
    }

    None
}

pub(super) fn normalize_dirty_paths(raw_paths: &[String]) -> HashSet<String> {
    raw_paths
        .iter()
        .map(|path| path.trim())
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub(super) fn build_semantic_path_index(root: &SemanticNode) -> HashMap<String, Vec<usize>> {
    let mut counts = HashMap::new();
    count_semantic_paths(root, "root", &mut counts);

    let mut index = HashMap::new();
    let mut current_child_path = Vec::new();
    collect_unique_semantic_paths(root, "root", &counts, &mut current_child_path, &mut index);
    index
}

fn count_semantic_paths(
    node: &SemanticNode,
    parent_path: &str,
    counts: &mut HashMap<String, usize>,
) {
    let current_path = semantic_path(parent_path, &node.role);
    *counts.entry(current_path.clone()).or_insert(0) += 1;

    for child in &node.children {
        count_semantic_paths(child, &current_path, counts);
    }
}

fn collect_unique_semantic_paths(
    node: &SemanticNode,
    parent_path: &str,
    counts: &HashMap<String, usize>,
    current_child_path: &mut Vec<usize>,
    index: &mut HashMap<String, Vec<usize>>,
) {
    let current_path = semantic_path(parent_path, &node.role);
    if counts.get(&current_path).copied() == Some(1) {
        index.insert(current_path.clone(), current_child_path.clone());
    }

    for (child_index, child) in node.children.iter().enumerate() {
        current_child_path.push(child_index);
        collect_unique_semantic_paths(child, &current_path, counts, current_child_path, index);
        current_child_path.pop();
    }
}

fn semantic_path(parent_path: &str, role: &str) -> String {
    format!("{}/{}", parent_path, role.to_lowercase())
}

pub(super) fn collect_stable_key_entries(
    node: &SemanticNode,
    out: &mut HashMap<String, StableKeyIndexEntry>,
) {
    if let Some(key) = &node.stable_key {
        if node.backend_node_id > 0 {
            // Index by the short key so that agents (which receive the 16-char form
            // from ExternalInteractiveElement) can resolve their key back to a node id.
            let short_key: String = key.chars().take(STABLE_KEY_SHORT_LEN).collect();
            out.insert(
                short_key,
                StableKeyIndexEntry {
                    backend_node_id: node.backend_node_id,
                    alias: node.alias.clone(),
                },
            );
        }
    }

    for child in &node.children {
        collect_stable_key_entries(child, out);
    }
}

pub(super) fn quad_to_bbox(quad: &[f64]) -> Option<[f64; 4]> {
    if quad.len() < 8 {
        return None;
    }

    let xs = [quad[0], quad[2], quad[4], quad[6]];
    let ys = [quad[1], quad[3], quad[5], quad[7]];

    let min_x = xs.iter().copied().fold(f64::INFINITY, f64::min);
    let max_x = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let min_y = ys.iter().copied().fold(f64::INFINITY, f64::min);
    let max_y = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max);

    if !min_x.is_finite() || !max_x.is_finite() || !min_y.is_finite() || !max_y.is_finite() {
        return None;
    }

    Some([
        min_x,
        min_y,
        (max_x - min_x).max(0.0),
        (max_y - min_y).max(0.0),
    ])
}

pub(super) fn normalize_text(input: &str) -> String {
    input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub(super) fn node_matches_state(node: &SemanticNode, desired_state: SemanticWaitState) -> bool {
    match desired_state {
        SemanticWaitState::Enabled => node_is_enabled(node),
    }
}

pub(super) fn node_is_enabled(node: &SemanticNode) -> bool {
    !node
        .attributes
        .as_ref()
        .is_some_and(|attrs| attrs.contains_key("disabled"))
}

pub(super) fn state_contains_intent(root: &SemanticNode, intent: &str) -> bool {
    let normalized_intent = intent.trim().to_lowercase();
    node_contains_intent(root, &normalized_intent)
}

fn node_contains_intent(node: &SemanticNode, intent: &str) -> bool {
    if node
        .label
        .as_deref()
        .is_some_and(|label| label.trim().to_lowercase() == intent)
    {
        return true;
    }

    if let Some(attrs) = &node.attributes {
        for (key, value) in attrs {
            let key_normalized = key.to_lowercase();
            let value_normalized = value.to_lowercase();
            if (key_normalized == "data-intent" || key_normalized == "intent")
                && value_normalized == intent
            {
                return true;
            }
        }
    }

    for child in &node.children {
        if node_contains_intent(child, intent) {
            return true;
        }
    }

    false
}

pub(super) fn error_chain_contains_any(err: &anyhow::Error, markers: &[&str]) -> bool {
    err.chain().any(|source| {
        let message = source.to_string().to_lowercase();
        markers.iter().any(|marker| message.contains(marker))
    })
}

pub(super) fn is_transient_capture_error(err: &anyhow::Error) -> bool {
    let transient_markers = [
        "could not find node",
        "no node with given id",
        "execution context was destroyed",
        "cannot find context with specified id",
        "navigation",
    ];

    error_chain_contains_any(err, &transient_markers)
}

pub(super) fn remaining_timeout(started: Instant, timeout: Duration) -> Duration {
    timeout.saturating_sub(started.elapsed())
}

pub(super) fn normalized_poll_interval(poll_interval: Duration) -> Duration {
    if poll_interval.is_zero() {
        DEFAULT_WAIT_POLL_INTERVAL
    } else {
        poll_interval
    }
}

pub(super) fn transient_error_backoff(poll_interval: Duration) -> Duration {
    min(
        normalized_poll_interval(poll_interval),
        MAX_TRANSIENT_ERROR_BACKOFF,
    )
}

pub(super) fn navigation_fallback_condition_met(
    requested_url: &str,
    previous_url: Option<&str>,
    current_url: Option<&str>,
    ready_state: Option<&str>,
    dom_non_empty: bool,
) -> bool {
    let reached_requested_url = current_url.is_some_and(|url| url == requested_url);
    let moved_to_new_url = match (previous_url, current_url) {
        (_, None) => false,
        (Some(previous), Some(current)) => current != previous,
        (None, Some(_)) => true,
    };
    let dom_ready = matches!(ready_state, Some("interactive" | "complete"));

    (reached_requested_url || moved_to_new_url) && (dom_ready || dom_non_empty)
}

pub(super) fn sleep_transient_backoff(
    started: Instant,
    timeout: Duration,
    poll_interval: Duration,
) {
    let remaining = remaining_timeout(started, timeout);
    if remaining.is_zero() {
        return;
    }

    thread::sleep(min(poll_interval, remaining));
}

pub(super) fn value_to_u64(value: &serde_json::Value) -> Result<u64> {
    if let Some(v) = value.as_u64() {
        return Ok(v);
    }

    if let Some(v) = value.as_i64() {
        return u64::try_from(v).context("Expected non-negative integer value");
    }

    anyhow::bail!("Expected integer value, received: {value}");
}

pub(super) fn value_to_string_vec(value: &serde_json::Value) -> Result<Vec<String>> {
    let entries = value
        .as_array()
        .context("Expected array value for dirty semantic paths")?;

    Ok(entries
        .iter()
        .filter_map(|entry| entry.as_str())
        .map(ToOwned::to_owned)
        .collect())
}

pub(super) fn duration_to_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

pub(super) fn epoch_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// Epoch milliseconds truncated to u64 for audit event timestamps.
/// Safe until ~year 584 million. Debug-asserts no truncation occurred.
pub(super) fn epoch_millis_u64() -> u64 {
    let ms = epoch_millis();
    debug_assert!(ms <= u128::from(u64::MAX), "epoch_millis overflowed u64");
    ms as u64
}
