// NOTE: This is a simplified educational example. The actual MCP server (mcp-server/src/lib.rs)
// includes additional attribute normalization and policy_flags inference.

//! Dragon Head — Developer Quickstart
//!
//! Demonstrates the core data model without a real browser:
//!   1. Build a SemanticState from fixture data
//!   2. Extract Fast State (interactive elements + messages)
//!   3. Evaluate a PolicyRule against a context
//!   4. Serialize the state as a `get_state` MCP-style JSON payload
//!
//! Run:
//!   cargo run --example quickstart
//!
//! No external credentials or running Chrome required.

use core_runtime::{
    policy::{PolicyAction, PolicyContext, PolicyEngine, PolicyRule},
    LoadProfile, SemanticNode, SemanticState,
};
use serde_json::json;
use std::collections::BTreeMap;

fn main() {
    println!("=== Dragon Head Quickstart ===\n");

    // ──────────────────────────────────────────────
    // 1. Build a SemanticState from fixture data
    //    (no browser required — we construct the tree
    //     directly from Rust structs)
    // ──────────────────────────────────────────────
    let state = build_fixture_state();
    println!("[1] SemanticState constructed");
    println!("    page_instance_id : {}", state.page_instance_id());
    println!("    state_hash       : {}", state.state_hash());
    println!("    load_profile     : {:?}", state.load_profile());
    println!();

    // ──────────────────────────────────────────────
    // 2. Generate Fast State
    //    Fast State = interactive_elements + messages
    //    (SPEC SRE-01: generated within 50 ms)
    // ──────────────────────────────────────────────
    let fast = state.generate_fast_state();
    println!("[2] Fast State generated");
    println!(
        "    interactive_elements : {}",
        fast.interactive_elements.len()
    );
    println!("    messages             : {}", fast.messages.len());
    for el in &fast.interactive_elements {
        println!(
            "      → role={:8}  alias={:25}  stable_key={}",
            el.role,
            el.alias.as_deref().unwrap_or("-"),
            el.stable_key.as_deref().unwrap_or("(none)"),
        );
    }
    println!();

    // ──────────────────────────────────────────────
    // 3. Policy Engine — block navigation to a domain
    // ──────────────────────────────────────────────
    let engine = build_policy_engine();
    println!(
        "[3] PolicyEngine loaded with {} rule(s)",
        engine.rules().len()
    );

    let allowed_ctx = PolicyContext {
        url: "https://safe.example.com/dashboard".to_string(),
        action: "navigate".to_string(),
        target_role: None,
        target_text: None,
        surrounding_text: None,
    };
    let blocked_ctx = PolicyContext {
        url: "https://blocked-domain.example.com/page".to_string(),
        action: "navigate".to_string(),
        target_role: None,
        target_text: None,
        surrounding_text: None,
    };

    let allowed_decision = engine.evaluate(&allowed_ctx);
    let blocked_decision = engine.evaluate(&blocked_ctx);

    println!("    safe.example.com       → {:?}", allowed_decision.action);
    println!(
        "    blocked-domain.example.com → {:?}",
        blocked_decision.action
    );
    println!();

    // ──────────────────────────────────────────────
    // 4. Serialize as MCP get_state response JSON
    // ──────────────────────────────────────────────
    let mcp_payload = build_mcp_get_state_response(&state);
    println!("[4] MCP get_state response (JSON):");
    println!(
        "{}",
        serde_json::to_string_pretty(&mcp_payload).expect("serialization failed")
    );

    println!("\n=== Done — no credentials required ===");
}

/// Construct a SemanticState that represents a simple checkout page.
fn build_fixture_state() -> SemanticState {
    let checkout_button = SemanticNode {
        role: "button".to_string(),
        label: Some("Purchase".to_string()),
        alias: Some("btn_purchase".to_string()),
        stable_key: Some(
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2".to_string(),
        ),
        attributes: Some({
            let mut m = BTreeMap::new();
            m.insert("disabled".to_string(), "false".to_string());
            m
        }),
        children: vec![],
        ambiguous: false,
        backend_node_id: 42,
    };

    let email_input = SemanticNode {
        role: "input".to_string(),
        label: Some("Email address".to_string()),
        alias: Some("input_email".to_string()),
        stable_key: Some(
            "b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3".to_string(),
        ),
        attributes: Some({
            let mut m = BTreeMap::new();
            m.insert("type".to_string(), "email".to_string());
            m.insert("required".to_string(), "true".to_string());
            m
        }),
        children: vec![],
        ambiguous: false,
        backend_node_id: 43,
    };

    let page_heading = SemanticNode {
        role: "text".to_string(),
        label: Some("Checkout".to_string()),
        alias: Some("h1_checkout".to_string()),
        stable_key: Some(
            "c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4".to_string(),
        ),
        attributes: None,
        children: vec![],
        ambiguous: false,
        backend_node_id: 10,
    };

    let root = SemanticNode {
        role: "document".to_string(),
        label: Some("Checkout page".to_string()),
        alias: Some("document".to_string()),
        stable_key: None,
        attributes: None,
        children: vec![page_heading, email_input, checkout_button],
        ambiguous: false,
        backend_node_id: 0,
    };

    SemanticState::new(root, LoadProfile::Minimal)
}

/// Build a minimal PolicyEngine that blocks navigation to a specific domain.
fn build_policy_engine() -> PolicyEngine {
    let rules = vec![PolicyRule {
        id: "block-navigation-to-restricted-domain".to_string(),
        domain: Some("blocked-domain.example.com".to_string()),
        path_prefix: None,
        role: None,
        text_regex: None,
        context_regex: None,
        action: PolicyAction::Block,
        scope: None,
        outcome_projector: None,
    }];

    PolicyEngine::try_new(rules).expect("policy rules should be valid")
}

/// Produce a JSON structure that mirrors the MCP `get_state` response shape
/// defined in SPEC §5.2 / mcp-server/src/lib.rs (ExternalSemanticState).
fn build_mcp_get_state_response(state: &SemanticState) -> serde_json::Value {
    let fast = state.generate_fast_state();

    let elements: Vec<serde_json::Value> = fast
        .interactive_elements
        .iter()
        .enumerate()
        .map(|(idx, node)| {
            json!({
                "id": node.backend_node_id,
                "stable_key": node.stable_key.as_deref().unwrap_or(""),
                "alias": node.alias.as_deref().unwrap_or(&format!("element_{idx}")),
                "role": node.role,
                "name": node.label.as_deref().unwrap_or(""),
                "attributes": node.attributes.as_ref().map(|m| {
                    m.iter().map(|(k, v)| (k.clone(), json!(v))).collect::<serde_json::Map<_,_>>()
                }).unwrap_or_default(),
                "bbox": [0.0, 0.0, 0.0, 0.0],
                "policy_flags": []
            })
        })
        .collect();

    json!({
        "metadata": {
            "url": "https://example.com/checkout",
            "page_instance_id": state.page_instance_id(),
            "state_hash": state.state_hash(),
            "load_profile": format!("{:?}", state.load_profile()).to_lowercase(),
            "timestamp": state.timestamp()
        },
        "interactive_elements": elements
    })
}
