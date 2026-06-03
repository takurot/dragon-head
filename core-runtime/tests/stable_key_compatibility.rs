use core_runtime::sre::StableKeyGenerator;
use core_runtime::sre::{
    normalize_dom, normalize_dom_with_viewport, normalize_dom_with_viewport_and_refinement,
    LoadProfile, SubtreeRefinementConfig, ViewportDimensions,
};
use core_runtime::{BrowserClient, PageSession};
use serde_json::json;
use std::collections::{HashMap, HashSet};

// ---------------------------------------------------------------------------
// Helper builders (mirror normalization.rs test helpers)
// ---------------------------------------------------------------------------

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

/// Build a minimal document with one div at a given pixel position via inline style.
fn build_positioned_document(
    left_px: u32,
    top_px: u32,
) -> headless_chrome::protocol::cdp::DOM::Node {
    let style = format!("position:absolute;left:{}px;top:{}px", left_px, top_px);
    let div = make_element_node(
        104,
        "div",
        vec![("style", &style)],
        vec![make_text_node(105, "Hello")],
    );
    let body = make_element_node(103, "body", vec![], vec![div]);
    let html = make_element_node(102, "html", vec![], vec![body]);
    make_document_node(101, vec![html])
}

#[test]
fn test_stable_key_hash_compatibility_vectors() {
    // NOTE: These vectors lock current hash behavior to detect incompatible changes.
    let mut generator = StableKeyGenerator::new();

    let (first_key, first_ambiguous) = generator.generate_key(
        "button",
        Some(" Submit "),
        "root/#document/html/body/button",
        "Top_Left",
    );
    assert_eq!(
        first_key,
        "14b454ad4839240ded4161ce1cc11568b1b7b1531d970a6b8905f9aad648dbe9"
    );
    assert!(!first_ambiguous);

    let (second_key, second_ambiguous) = generator.generate_key(
        "button",
        Some(" Submit "),
        "root/#document/html/body/button",
        "Top_Left",
    );
    assert_eq!(
        second_key,
        "0077d9753c502853a96acdfdb031e6f48e6f30fc9681273b3224f075e749e032"
    );
    assert!(second_ambiguous);

    let (third_key, third_ambiguous) = generator.generate_key(
        "button",
        Some(" Submit "),
        "root/#document/html/body/button",
        "Top_Left",
    );
    assert_eq!(
        third_key,
        "b655a548c46f03ca01c9d10cf0eda7f774cb278a6a3ff499735a7d459750ca01"
    );
    assert!(third_ambiguous);
}

#[test]
fn test_stable_key_hash_compatibility_none_label_vector() {
    let mut generator = StableKeyGenerator::new();
    let (key, ambiguous) = generator.generate_key(
        "input",
        None,
        "root/#document/html/body/form/input",
        "Bottom_Right",
    );
    assert_eq!(
        key,
        "2a91f3b04cc2ea295b036f89c810fde7314880b0f60810d9a1db55b9fab4561e"
    );
    assert!(!ambiguous);
}

#[test]
fn test_stable_key_hash_compatibility_non_ascii_label_vector() {
    // Guard Unicode lowercasing/trim path from accidental compatibility regressions.
    let mut generator = StableKeyGenerator::new();
    let (key, ambiguous) = generator.generate_key(
        "button",
        Some("  İSTANBUL Straße  "),
        "root/#document/html/body/button[1]",
        "Top_Right",
    );
    assert_eq!(
        key,
        "c64f7d577b24a91e8879f60c3a2e4b88e1b3c1fbbdac8dbc09ee6757af252286"
    );
    assert!(!ambiguous);
}

// ---------------------------------------------------------------------------
// Issue #49: viewport-aware quadrant tests
// ---------------------------------------------------------------------------

/// Normalization without explicit viewport must produce identical output to the
/// pre-existing default (800×600) behavior.
#[test]
fn test_normalize_dom_default_viewport_unchanged() {
    let dom = build_positioned_document(100, 100); // top-left quadrant in 800×600
    let default_result = normalize_dom(LoadProfile::Minimal, &dom).unwrap();
    let explicit_default = normalize_dom_with_viewport(
        LoadProfile::Minimal,
        &dom,
        ViewportDimensions {
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    // The stable_key must be identical because the viewport matches the default constants.
    let body = &default_result.children[0].children[0];
    let body_explicit = &explicit_default.children[0].children[0];
    assert_eq!(
        body.children[0].stable_key, body_explicit.children[0].stable_key,
        "explicit 800×600 viewport must produce the same stable_key as the default"
    );
}

/// An element at (450, 350) lands in bottom-right for 800×600 (x=450>400, y=350>300),
/// but top-right in 1920×1080 (x=450<960, y=350<540).
#[test]
fn test_explicit_viewport_used_for_quadrant_calculation() {
    let dom = build_positioned_document(450, 350);

    let result_desktop = normalize_dom_with_viewport(
        LoadProfile::Minimal,
        &dom,
        ViewportDimensions {
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    let result_wide = normalize_dom_with_viewport(
        LoadProfile::Minimal,
        &dom,
        ViewportDimensions {
            width: 1920.0,
            height: 1080.0,
        },
    )
    .unwrap();

    let key_desktop = result_desktop.children[0].children[0].children[0]
        .stable_key
        .clone()
        .unwrap();
    let key_wide = result_wide.children[0].children[0].children[0]
        .stable_key
        .clone()
        .unwrap();

    assert_ne!(
        key_desktop, key_wide,
        "different viewports should yield different quadrant → different stable_key"
    );
}

/// Desktop (1920×1080) vs mobile (375×812): element at (450, 100).
/// Desktop: x=450 < 960 → left, y=100 < 540 → top ⇒ top_left
/// Mobile:  x=450 > 187.5 → right, y=100 < 406 → top ⇒ top_right
#[test]
fn test_desktop_vs_mobile_viewport_different_quadrant() {
    let dom = build_positioned_document(450, 100);

    let result_desktop = normalize_dom_with_viewport(
        LoadProfile::Minimal,
        &dom,
        ViewportDimensions {
            width: 1920.0,
            height: 1080.0,
        },
    )
    .unwrap();

    let result_mobile = normalize_dom_with_viewport(
        LoadProfile::Minimal,
        &dom,
        ViewportDimensions {
            width: 375.0,
            height: 812.0,
        },
    )
    .unwrap();

    let key_desktop = result_desktop.children[0].children[0].children[0]
        .stable_key
        .clone()
        .unwrap();
    let key_mobile = result_mobile.children[0].children[0].children[0]
        .stable_key
        .clone()
        .unwrap();

    assert_ne!(
        key_desktop, key_mobile,
        "desktop 1920×1080 and mobile 375×812 should yield different stable_keys for a straddling coordinate"
    );
}

/// An element at (100, 100) is in the top-left quadrant of 800×600.
/// The stable_key from explicit 800×600 viewport must match the default.
#[test]
fn test_top_left_quadrant_for_small_coordinate() {
    let dom = build_positioned_document(100, 100);
    let result = normalize_dom_with_viewport(
        LoadProfile::Minimal,
        &dom,
        ViewportDimensions {
            width: 800.0,
            height: 600.0,
        },
    )
    .unwrap();

    let default_result = normalize_dom(LoadProfile::Minimal, &dom).unwrap();
    let key_explicit = result.children[0].children[0].children[0]
        .stable_key
        .clone()
        .unwrap();
    let key_default = default_result.children[0].children[0].children[0]
        .stable_key
        .clone()
        .unwrap();
    assert_eq!(
        key_explicit, key_default,
        "top-left coordinate with explicit 800×600 must match default behavior"
    );
}

// ---------------------------------------------------------------------------
// Issue #65: viewport change must invalidate refinement-cached subtrees
// ---------------------------------------------------------------------------

/// Helper: build a cached path index for the positioned document returned by
/// `build_positioned_document`.  The tree is:
///   #document > html > body > div (with style) > #text
/// Unique paths are all of them (no duplicate roles at any level).
fn build_positioned_path_index(
    root: &core_runtime::sre::SemanticNode,
) -> HashMap<String, Vec<usize>> {
    // Manually encode the index for the four-node chain.
    // root/#document          -> []
    // root/#document/html     -> [0]
    // root/#document/html/body-> [0,0]
    // root/#document/html/body/div -> [0,0,0]
    // (text nodes are not indexed in the semantic path index)
    let _ = root; // suppress unused warning; index is structure-driven
    HashMap::from([
        ("root/#document".to_string(), vec![]),
        ("root/#document/html".to_string(), vec![0]),
        ("root/#document/html/body".to_string(), vec![0, 0]),
        ("root/#document/html/body/div".to_string(), vec![0, 0, 0]),
    ])
}

/// Regression test for PR #65: when viewport changes between captures, the
/// refinement path must recompute stable keys for ALL nodes — not just dirty
/// ones — because quadrant calculation depends on viewport dimensions.
///
/// Scenario:
///   1. First capture at default 800×600 viewport.
///   2. Viewport changes to 375×812 (mobile emulation).
///   3. A second capture uses the refinement path with an EMPTY dirty-path set
///      (i.e., no DOM mutations were observed).
///   4. The stable key for the positioned div must equal a fresh full capture
///      at 375×812 — NOT the cached key from the 800×600 pass.
#[test]
fn test_refinement_cache_invalidated_on_viewport_change() {
    // Element at (450, 100): top-right in 800×600 but also top-right in 375×812
    // with different x-ratio (450 > 187.5 → right).  Use an element that
    // straddles the midpoint differently so the stable key actually differs.
    //
    // Use (450, 350): bottom-right in 800×600 (x>400, y>300) but top-right in
    // 1920×1080 (x<960, y<540).  The stable key must reflect the new viewport.
    let dom = build_positioned_document(450, 350);

    // --- step 1: initial capture at default 800×600 ---
    let initial_800 =
        normalize_dom_with_viewport(LoadProfile::Minimal, &dom, ViewportDimensions::default())
            .unwrap();
    let cached_path_index = build_positioned_path_index(&initial_800);

    // --- step 2: second capture via refinement path, non-default viewport ---
    let new_viewport = ViewportDimensions {
        width: 1920.0,
        height: 1080.0,
    };
    // No dirty paths: if caching were used, ALL nodes would be served from cache.
    let empty_dirty: HashSet<String> = HashSet::new();

    let refined = normalize_dom_with_viewport_and_refinement(
        LoadProfile::Minimal,
        &dom,
        new_viewport,
        SubtreeRefinementConfig {
            dirty_paths: &empty_dirty,
            cached_paths: &cached_path_index,
            cached_root: &initial_800,
        },
    )
    .unwrap();

    // --- step 3: expected result — full fresh capture at new viewport ---
    let expected = normalize_dom_with_viewport(LoadProfile::Minimal, &dom, new_viewport).unwrap();

    // The div is at children[0].children[0].children[0]
    let refined_key = refined.children[0].children[0].children[0]
        .stable_key
        .clone()
        .unwrap();
    let expected_key = expected.children[0].children[0].children[0]
        .stable_key
        .clone()
        .unwrap();
    let cached_key = initial_800.children[0].children[0].children[0]
        .stable_key
        .clone()
        .unwrap();

    assert_eq!(
        refined_key, expected_key,
        "refinement path with non-default viewport must produce the same key as a fresh full capture"
    );
    assert_ne!(
        refined_key, cached_key,
        "refinement path must NOT reuse the cached key from the old viewport"
    );
}

// ---------------------------------------------------------------------------
// Browser-backed smoke test: actual viewport from running Chrome
// Skipped automatically if Chrome is not available (CI-friendly).
// ---------------------------------------------------------------------------

/// Smoke test: start two real Chrome instances with different window sizes
/// (desktop 1280x720 vs mobile 375x812), load the same fixture HTML, and
/// confirm that an element straddling a quadrant boundary gets a different
/// stable_key in each run.
///
/// This is the automated equivalent of the manual smoke test:
/// "navigate to a page with a wide viewport and confirm stable_key quadrant
/// differs from a narrow viewport for straddling elements."
#[test]
fn test_browser_backed_viewport_affects_stable_key() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    // HTML fixture: a button at left:300px, top:200px.
    //
    // Quadrant math (x_ratio = left_px / window.innerWidth):
    //   desktop  1280×720 → 300/1280 = 0.234 < 0.5 → Left half  → Top_Left
    //   mobile    375×812 → 300/ 375 = 0.800 ≥ 0.5 → Right half → Top_Right
    //
    // left:640px would be ambiguous: 640/1280 = 0.5 (boundary → Right) and
    // 640/375 = 1.71 (also Right), so both viewports give the same quadrant.
    let fixture_html = r#"<!DOCTYPE html><html><body style="margin:0;padding:0;">
<button id="cta"
        style="position:absolute;left:300px;top:200px;width:120px;height:40px;">
  Buy now
</button>
</body></html>"#;

    let url = format!("data:text/html,{}", urlencoding::encode(fixture_html));

    // --- desktop viewport (1280×720): 300/1280 = 0.234 → Left half → Top_Left ---
    let desktop = BrowserClient::new_with_window_size(1280, 720)?;
    let desktop_page = desktop.new_page()?;
    desktop_page.navigate(&url)?;
    assert_eq!(evaluate_viewport(&desktop_page)?, (1280, 720));
    let desktop_state = desktop_page.capture_semantic_state(LoadProfile::Interactive)?;

    // --- mobile viewport (375×812): 300/375 = 0.800 → Right half → Top_Right ---
    let mobile = BrowserClient::new_with_window_size(375, 812)?;
    let mobile_page = mobile.new_page()?;
    mobile_page.navigate(&url)?;
    assert_eq!(evaluate_viewport(&mobile_page)?, (375, 812));
    let mobile_state = mobile_page.capture_semantic_state(LoadProfile::Interactive)?;

    // Extract the stable_key for the "Buy now" button in both captures.
    fn find_button_key(state: &core_runtime::sre::SemanticState) -> Option<String> {
        fn search(node: &core_runtime::sre::SemanticNode) -> Option<String> {
            if node.role.as_str() == "button" {
                return node.stable_key.clone();
            }
            for child in &node.children {
                if let Some(k) = search(child) {
                    return Some(k);
                }
            }
            None
        }
        search(state.root())
    }

    let desktop_key =
        find_button_key(&desktop_state).expect("desktop capture must find the button");
    let mobile_key = find_button_key(&mobile_state).expect("mobile capture must find the button");

    assert_ne!(
        desktop_key, mobile_key,
        "stable_key for an element straddling a quadrant boundary must differ \
         between desktop (1280x720) and mobile (375x812) viewports\n\
         desktop key: {desktop_key}\n\
         mobile  key: {mobile_key}"
    );

    Ok(())
}

fn evaluate_viewport(page: &PageSession) -> anyhow::Result<(u32, u32)> {
    let value = page.evaluate_script(
        "JSON.stringify({width: window.innerWidth, height: window.innerHeight})",
    )?;
    let raw = value
        .value
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .ok_or_else(|| anyhow::anyhow!("viewport script did not return a JSON string"))?;
    let parsed: serde_json::Value = serde_json::from_str(&raw)?;
    let width = parsed
        .get("width")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| anyhow::anyhow!("viewport width was missing"))?;
    let height = parsed
        .get("height")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| anyhow::anyhow!("viewport height was missing"))?;
    Ok((width as u32, height as u32))
}
