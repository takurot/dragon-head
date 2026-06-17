/// Regression tests: stable_key stability under prompt-injection sanitization (ISSUE-111).
///
/// Acceptance criteria:
///   AC1 — ReportOnly mode does not change stable keys for an unchanged DOM.
///   AC2 — Redact mode does not create avoidable stable_key collisions from
///          repeated `[REDACTED_SECURITY]` labels.
///   AC3 — Existing stable-key compatibility tests continue to pass (verified by CI).
use std::time::Duration;

use core_runtime::prompt_injection::REDACTION_PLACEHOLDER;
use core_runtime::sre::{
    normalize_dom, AsyncPipeline, AsyncPipelineConfig, LoadProfile, PromptInjectionMode,
    SemanticNode, SemanticState, StableKeyGenerator,
};
use serde_json::json;

// ── DOM builder helpers (mirrors stable_key_compatibility.rs) ─────────────────

fn make_document_node(
    node_id: u32,
    children: Vec<headless_chrome::protocol::cdp::DOM::Node>,
) -> headless_chrome::protocol::cdp::DOM::Node {
    serde_json::from_value(json!({
        "nodeId": node_id,
        "backendNodeId": node_id,
        "nodeType": 9,
        "nodeName": "#document",
        "localName": "",
        "nodeValue": "",
        "childNodeCount": children.len(),
        "children": children
    }))
    .unwrap()
}

fn make_element_node(
    node_id: u32,
    tag: &str,
    attributes: Vec<(&str, &str)>,
    children: Vec<headless_chrome::protocol::cdp::DOM::Node>,
) -> headless_chrome::protocol::cdp::DOM::Node {
    let mut flat_attrs: Vec<String> = Vec::with_capacity(attributes.len() * 2);
    for (key, value) in attributes {
        flat_attrs.push(key.to_string());
        flat_attrs.push(value.to_string());
    }
    serde_json::from_value(json!({
        "nodeId": node_id,
        "backendNodeId": node_id,
        "nodeType": 1,
        "nodeName": tag.to_uppercase(),
        "localName": tag,
        "nodeValue": "",
        "attributes": flat_attrs,
        "childNodeCount": children.len(),
        "children": children
    }))
    .unwrap()
}

fn make_text_node(node_id: u32, text: &str) -> headless_chrome::protocol::cdp::DOM::Node {
    serde_json::from_value(json!({
        "nodeId": node_id,
        "backendNodeId": node_id,
        "nodeType": 3,
        "nodeName": "#text",
        "localName": "",
        "nodeValue": text
    }))
    .unwrap()
}

// ── SemanticState helpers ─────────────────────────────────────────────────────

fn pipeline_with_mode(mode: PromptInjectionMode) -> AsyncPipeline {
    AsyncPipeline::new(AsyncPipelineConfig {
        injection_mode: mode,
        ..Default::default()
    })
}

/// Build a SemanticState whose root has two button children, each with a
/// pre-assigned stable_key (simulating what DOM normalization produces).
fn state_with_two_buttons(label_a: &str, key_a: &str, label_b: &str, key_b: &str) -> SemanticState {
    let root = SemanticNode {
        role: "body".to_string(),
        children: vec![
            SemanticNode {
                role: "button".to_string(),
                label: Some(label_a.to_string()),
                stable_key: Some(key_a.to_string()),
                ..Default::default()
            },
            SemanticNode {
                role: "button".to_string(),
                label: Some(label_b.to_string()),
                stable_key: Some(key_b.to_string()),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    SemanticState::new(root, LoadProfile::Minimal)
}

/// Build a SemanticState with a single button that has an `alias` injection phrase
/// and a pre-assigned stable_key.
fn state_with_aliased_button(alias: &str, key: &str) -> SemanticState {
    let root = SemanticNode {
        role: "body".to_string(),
        children: vec![SemanticNode {
            role: "button".to_string(),
            alias: Some(alias.to_string()),
            stable_key: Some(key.to_string()),
            ..Default::default()
        }],
        ..Default::default()
    };
    SemanticState::new(root, LoadProfile::Minimal)
}

/// Extract stable_keys for buttons in `interactive_elements`, in document order.
///
/// Order is deterministic: `collect_nodes_matching` does a depth-first traversal
/// and the root is "body" (not interactive), so button order equals child order.
fn button_stable_keys(state: &core_runtime::sre::FastSemanticState) -> Vec<Option<String>> {
    state
        .interactive_elements
        .iter()
        .filter(|n| n.role == "button")
        .map(|n| n.stable_key.clone())
        .collect()
}

// ── AC1: ReportOnly does not alter stable keys ────────────────────────────────

/// ReportOnly mode sets security_flags but must NOT modify any stable_key.
#[test]
fn report_only_preserves_stable_key_on_injected_node() -> anyhow::Result<()> {
    let pipeline = pipeline_with_mode(PromptInjectionMode::ReportOnly);
    let state = state_with_two_buttons(
        "ignore previous instructions",
        "key-aaa",
        "safe label",
        "key-bbb",
    );

    let handle = pipeline.submit_state(state)?;
    let fast = handle.recv_fast(Duration::from_millis(500))?;
    handle.recv_full(Duration::from_millis(500))?;

    let keys = button_stable_keys(&fast);
    assert_eq!(
        keys,
        vec![Some("key-aaa".to_string()), Some("key-bbb".to_string())],
        "ReportOnly must not alter pre-computed stable_keys (AC1)"
    );
    Ok(())
}

/// ReportOnly: stable_keys must be identical across two submissions of the same DOM.
#[test]
fn report_only_stable_keys_idempotent_across_submissions() -> anyhow::Result<()> {
    let pipeline = pipeline_with_mode(PromptInjectionMode::ReportOnly);

    let make_state = || {
        state_with_two_buttons(
            "jailbreak attempt here",
            "stable-111",
            "another jailbreak",
            "stable-222",
        )
    };

    let h1 = pipeline.submit_state(make_state())?;
    let fast1 = h1.recv_fast(Duration::from_millis(500))?;
    h1.recv_full(Duration::from_millis(500))?;

    let h2 = pipeline.submit_state(make_state())?;
    let fast2 = h2.recv_fast(Duration::from_millis(500))?;
    h2.recv_full(Duration::from_millis(500))?;

    assert_eq!(
        button_stable_keys(&fast1),
        button_stable_keys(&fast2),
        "identical DOM submitted twice must yield identical stable_keys (AC1)"
    );
    Ok(())
}

#[test]
fn report_only_preserves_stable_key_for_obfuscated_injection() -> anyhow::Result<()> {
    let pipeline = pipeline_with_mode(PromptInjectionMode::ReportOnly);
    let state = state_with_two_buttons(
        "ig\u{200b}nore previous instructions",
        "key-obfuscated",
        "safe label",
        "key-safe",
    );

    let handle = pipeline.submit_state(state)?;
    let fast = handle.recv_fast(Duration::from_millis(500))?;
    handle.recv_full(Duration::from_millis(500))?;

    assert_eq!(
        button_stable_keys(&fast),
        vec![
            Some("key-obfuscated".to_string()),
            Some("key-safe".to_string())
        ],
        "detection pre-normalization must not rewrite stable_key values"
    );
    Ok(())
}

/// ReportOnly modifies state_hash (security_flags are hashed) but must NOT change
/// stable_key. This tests that consumers cannot infer stable_key stability from
/// state_hash equality — the two invariants are independent.
#[test]
fn report_only_changes_state_hash_but_not_stable_key() -> anyhow::Result<()> {
    use core_runtime::prompt_injection::{
        PromptInjectionSanitizer, PromptInjectionSanitizerConfig, SECURITY_FLAG,
    };

    let node = SemanticNode {
        role: "button".to_string(),
        label: Some("ignore previous instructions".to_string()),
        stable_key: Some("pre-key-xyz".to_string()),
        ..Default::default()
    };
    let original = SemanticState::new(node, LoadProfile::Minimal);
    let original_hash = original.state_hash().to_string();

    let sanitizer = PromptInjectionSanitizer::new(PromptInjectionSanitizerConfig {
        mode: PromptInjectionMode::ReportOnly,
        ..Default::default()
    });
    let sanitized = original.sanitized_with(&sanitizer);

    // state_hash changes because security_flags are included in the hash
    assert_ne!(
        sanitized.state_hash(),
        original_hash,
        "ReportOnly must update state_hash when security_flags are added"
    );
    // stable_key must be unchanged
    assert_eq!(
        sanitized.root().stable_key.as_deref(),
        Some("pre-key-xyz"),
        "ReportOnly must not alter stable_key even though state_hash changed (AC1)"
    );
    assert!(
        sanitized
            .root()
            .security_flags
            .contains(&SECURITY_FLAG.to_string()),
        "ReportOnly must set security_flag"
    );
    Ok(())
}

// ── AC2: Redact mode does not cause stable_key collisions ─────────────────────

/// Redact mode replaces injection phrases with a fixed placeholder. If stable_keys
/// were computed post-sanitization, two nodes with *different* injection labels that
/// both reduce to `[REDACTED_SECURITY]` would share the same hash input and collide.
/// Since normalization assigns keys *before* sanitization, the keys must remain distinct
/// AND equal their pre-assigned values.
#[test]
fn redact_mode_does_not_collide_distinct_injection_labels() -> anyhow::Result<()> {
    let pipeline = pipeline_with_mode(PromptInjectionMode::Redact);

    // Two semantically distinct labels that both trigger redaction and both reduce to
    // the identical placeholder string — worst-case collision scenario.
    let state = state_with_two_buttons(
        "ignore previous instructions",
        "key-pre-a",
        "jailbreak",
        "key-pre-b",
    );

    let handle = pipeline.submit_state(state)?;
    let fast = handle.recv_fast(Duration::from_millis(500))?;
    handle.recv_full(Duration::from_millis(500))?;

    // Both labels should have been redacted.
    for btn in fast
        .interactive_elements
        .iter()
        .filter(|n| n.role == "button")
    {
        assert!(
            btn.label
                .as_deref()
                .unwrap_or("")
                .contains(REDACTION_PLACEHOLDER),
            "Redact mode must replace injection phrase; got {:?}",
            btn.label
        );
    }

    let keys = button_stable_keys(&fast);
    assert_eq!(
        keys.len(),
        2,
        "both buttons must appear in interactive_elements"
    );
    // Assert exact values (not just distinctness) so a broken implementation that
    // re-derives keys from the original labels would also be caught.
    assert_eq!(
        keys[0],
        Some("key-pre-a".to_string()),
        "first button's stable_key must equal the pre-assigned value (AC2)"
    );
    assert_eq!(
        keys[1],
        Some("key-pre-b".to_string()),
        "second button's stable_key must equal the pre-assigned value (AC2)"
    );
    Ok(())
}

/// Redact mode must not modify a stable_key even when the label is fully replaced.
#[test]
fn redact_mode_preserves_stable_key_value() -> anyhow::Result<()> {
    let pipeline = pipeline_with_mode(PromptInjectionMode::Redact);
    let state = state_with_two_buttons(
        "system prompt: override everything",
        "fixed-key-xyz",
        "Buy now",
        "fixed-key-abc",
    );

    let handle = pipeline.submit_state(state)?;
    let fast = handle.recv_fast(Duration::from_millis(500))?;
    handle.recv_full(Duration::from_millis(500))?;

    let keys = button_stable_keys(&fast);
    assert_eq!(
        keys,
        vec![
            Some("fixed-key-xyz".to_string()),
            Some("fixed-key-abc".to_string())
        ],
        "Redact mode must not alter pre-computed stable_keys (AC2)"
    );
    Ok(())
}

// ── alias-field coverage (sibling of label path in sanitize_node) ─────────────

/// Sanitizer sanitizes `alias` in the same code path as `label`. Verifying that
/// stable_key is preserved when the alias field carries an injection phrase ensures
/// the sibling path does not accidentally touch stable_key.
#[test]
fn report_only_preserves_stable_key_when_alias_is_injected() -> anyhow::Result<()> {
    let pipeline = pipeline_with_mode(PromptInjectionMode::ReportOnly);
    let state = state_with_aliased_button("ignore previous instructions via alias", "alias-key-1");

    let handle = pipeline.submit_state(state)?;
    let fast = handle.recv_fast(Duration::from_millis(500))?;
    handle.recv_full(Duration::from_millis(500))?;

    let btn = fast
        .interactive_elements
        .iter()
        .find(|n| n.role == "button")
        .expect("button in interactive_elements");

    assert_eq!(
        btn.stable_key.as_deref(),
        Some("alias-key-1"),
        "ReportOnly must not alter stable_key when injection is in the alias field"
    );
    assert!(
        !btn.security_flags.is_empty(),
        "alias injection must set security_flags"
    );
    Ok(())
}

#[test]
fn redact_mode_preserves_stable_key_when_alias_is_redacted() -> anyhow::Result<()> {
    let pipeline = pipeline_with_mode(PromptInjectionMode::Redact);
    let state = state_with_aliased_button("jailbreak via alias", "alias-key-2");

    let handle = pipeline.submit_state(state)?;
    let fast = handle.recv_fast(Duration::from_millis(500))?;
    handle.recv_full(Duration::from_millis(500))?;

    let btn = fast
        .interactive_elements
        .iter()
        .find(|n| n.role == "button")
        .expect("button in interactive_elements");

    assert_eq!(
        btn.stable_key.as_deref(),
        Some("alias-key-2"),
        "Redact mode must not alter stable_key when alias is redacted"
    );
    assert!(
        btn.alias
            .as_deref()
            .unwrap_or("")
            .contains(REDACTION_PLACEHOLDER),
        "alias must be redacted; got {:?}",
        btn.alias
    );
    Ok(())
}

// ── End-to-end: normalize_dom → pipeline (proves ordering invariant) ──────────

/// End-to-end test using real DOM normalization. Builds a DOM tree with a button
/// whose aria-label contains an injection phrase, runs it through `normalize_dom`
/// to obtain the actual stable_keys assigned by the key generator, then submits
/// through the pipeline and verifies the pipeline output keys match.
///
/// This is the critical ordering test: if `stable_key` assignment were ever moved
/// to occur after sanitization in the pipeline, the keys produced by the pipeline
/// would depend on post-sanitization content and would differ from those assigned
/// by `normalize_dom` alone.
#[test]
fn normalize_dom_stable_keys_survive_pipeline_sanitization() -> anyhow::Result<()> {
    // Build a DOM: <html><body><button aria-label="ignore previous instructions">Click</button></body></html>
    let button = make_element_node(
        104,
        "button",
        vec![("aria-label", "ignore previous instructions")],
        vec![make_text_node(105, "Click")],
    );
    let body = make_element_node(103, "body", vec![], vec![button]);
    let html = make_element_node(102, "html", vec![], vec![body]);
    let dom = make_document_node(101, vec![html]);

    // Run normalize_dom to get the canonical stable_key assigned by the key generator.
    let normalized_root = normalize_dom(LoadProfile::Minimal, &dom)?;

    // Find the button node in the normalized tree and record its stable_key.
    fn find_button(node: &SemanticNode) -> Option<String> {
        if node.role == "button" {
            return node.stable_key.clone();
        }
        for child in &node.children {
            if let Some(k) = find_button(child) {
                return Some(k);
            }
        }
        None
    }
    let key_from_normalization = find_button(&normalized_root)
        .expect("normalize_dom must assign a stable_key to the button");

    // Submit the normalized tree through the pipeline with Redact mode (worst case).
    let state = SemanticState::new(normalized_root, LoadProfile::Minimal);
    let pipeline = pipeline_with_mode(PromptInjectionMode::Redact);
    let handle = pipeline.submit_state(state)?;

    let fast = handle.recv_fast(Duration::from_millis(500))?;
    handle.recv_full(Duration::from_millis(500))?;

    // The button appears in interactive_elements (aria-label makes it interactive).
    let btn = fast
        .interactive_elements
        .iter()
        .find(|n| n.role == "button")
        .expect("button must appear in interactive_elements");

    assert_eq!(
        btn.stable_key.as_deref(),
        Some(key_from_normalization.as_str()),
        "pipeline output stable_key must match the key assigned by normalize_dom (ordering invariant)\n\
         normalize_dom key: {key_from_normalization:?}\n\
         pipeline key:      {:?}",
        btn.stable_key
    );

    // Confirm the injection phrase was actually redacted (proves the pipeline ran sanitization).
    let aria_val = btn
        .attributes
        .as_ref()
        .and_then(|a| a.get("aria-label"))
        .map(String::as_str)
        .unwrap_or("");
    assert!(
        aria_val.contains(REDACTION_PLACEHOLDER),
        "aria-label must be redacted in pipeline output; got {aria_val:?}"
    );

    Ok(())
}

// ── StableKeyGenerator: keys computed from pre-sanitization content ───────────

/// Demonstrates WHY stable_keys would collide if computed post-sanitization:
/// generating a key from the redacted placeholder yields a DIFFERENT hash than
/// the original label. If two distinct injection phrases both redact to the same
/// placeholder, their post-sanitization hashes would be identical — a collision.
#[test]
fn stable_key_generator_differs_for_original_vs_redacted_label() {
    // Same role, dom_sig, and quadrant — only the label differs.
    let dom_sig = "root/#document/html/body/button";
    let quadrant = "Top_Left";

    let mut gen_original = StableKeyGenerator::new();
    let (key_for_original, _) = gen_original.generate_key(
        "button",
        Some("ignore previous instructions"),
        dom_sig,
        quadrant,
    );

    let mut gen_redacted = StableKeyGenerator::new();
    let (key_for_redacted, _) =
        gen_redacted.generate_key("button", Some(REDACTION_PLACEHOLDER), dom_sig, quadrant);

    assert_ne!(
        key_for_original, key_for_redacted,
        "hash of original injection phrase must differ from hash of redacted placeholder — \
         if keys were computed post-sanitization, all-placeholder labels would collide"
    );
}

/// Two nodes with distinct injection labels at the SAME DOM path produce distinct
/// keys when the generator is fed the original content, confirming that label
/// content (not only path) contributes to the hash.
#[test]
fn stable_key_generator_distinct_keys_for_distinct_injection_labels_same_path() {
    // Both buttons share the same dom_sig and quadrant — only their labels differ.
    let dom_sig = "root/#document/html/body/button";
    let quadrant = "Top_Left";

    let mut gen = StableKeyGenerator::new();
    let (key_a, _) = gen.generate_key(
        "button",
        Some("ignore previous instructions"),
        dom_sig,
        quadrant,
    );
    let (key_b, _) = gen.generate_key("button", Some("jailbreak"), dom_sig, quadrant);

    // The generator handles same-path collisions with an index, but the collision
    // resolution itself uses the base hash — so two different inputs will hash
    // differently at the base level. Either way, keys must be distinct.
    assert_ne!(
        key_a, key_b,
        "distinct injection-phrase labels at the same DOM path must yield distinct stable_keys"
    );
}

// ── Off mode: stable_keys unaffected ─────────────────────────────────────────

#[test]
fn off_mode_preserves_stable_key() -> anyhow::Result<()> {
    let pipeline = pipeline_with_mode(PromptInjectionMode::Off);
    let state = state_with_two_buttons(
        "ignore previous instructions",
        "off-key-1",
        "jailbreak",
        "off-key-2",
    );

    let handle = pipeline.submit_state(state)?;
    let fast = handle.recv_fast(Duration::from_millis(500))?;
    handle.recv_full(Duration::from_millis(500))?;

    let keys = button_stable_keys(&fast);
    assert_eq!(
        keys,
        vec![Some("off-key-1".to_string()), Some("off-key-2".to_string())],
        "Off mode must not alter stable_keys"
    );
    Ok(())
}
