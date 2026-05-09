/// Reproduction tests for ISSUE-03: Hardcoded viewport constants in quadrant calculation.
///
/// Before the fix, `normalize_dom` used hardcoded `DEFAULT_VIEWPORT_WIDTH_PX` (800) and
/// `DEFAULT_VIEWPORT_HEIGHT_PX` (600) to compute the quadrant embedded in `stable_key`.
/// This caused incorrect quadrant assignment whenever the actual browser viewport differed
/// from 800×600 — for example on a 1920×1080 desktop or a 375×812 mobile device.
///
/// The fix introduced:
/// 1. `normalize_dom_with_viewport` — accepts explicit `ViewportDimensions` so the caller
///    can pass the real browser dimensions, producing accurate quadrant values.
/// 2. `PageSession::get_viewport_dimensions` — reads `window.innerWidth/innerHeight` via
///    JavaScript and passes the result to every `normalize_dom_with_viewport` call, so
///    that `capture_semantic_state` always reflects the actual browser viewport.
use core_runtime::sre::{
    normalize_dom, normalize_dom_with_viewport, LoadProfile, ViewportDimensions,
};
use serde_json::json;

// ---------------------------------------------------------------------------
// Synthetic DOM helpers (same pattern as stable_key_compatibility.rs)
// ---------------------------------------------------------------------------

fn make_document_node(
    children: Vec<headless_chrome::protocol::cdp::DOM::Node>,
) -> headless_chrome::protocol::cdp::DOM::Node {
    serde_json::from_value(json!({
        "nodeId": 1, "backendNodeId": 1, "nodeType": 9,
        "nodeName": "#document", "localName": "", "nodeValue": "",
        "childNodeCount": children.len(), "children": children
    }))
    .unwrap()
}

fn make_element_node(
    node_id: u32,
    tag: &str,
    attributes: Vec<(&str, &str)>,
    children: Vec<headless_chrome::protocol::cdp::DOM::Node>,
) -> headless_chrome::protocol::cdp::DOM::Node {
    let mut flat: Vec<String> = Vec::with_capacity(attributes.len() * 2);
    for (k, v) in attributes {
        flat.push(k.to_string());
        flat.push(v.to_string());
    }
    serde_json::from_value(json!({
        "nodeId": node_id, "backendNodeId": node_id, "nodeType": 1,
        "nodeName": tag.to_uppercase(), "localName": tag, "nodeValue": "",
        "attributes": flat,
        "childNodeCount": children.len(), "children": children
    }))
    .unwrap()
}

fn make_text_node(node_id: u32, text: &str) -> headless_chrome::protocol::cdp::DOM::Node {
    serde_json::from_value(json!({
        "nodeId": node_id, "backendNodeId": node_id, "nodeType": 3,
        "nodeName": "#text", "localName": "", "nodeValue": text
    }))
    .unwrap()
}

/// Build a document with one `div` positioned at `(left_px, top_px)` via inline style.
fn positioned_doc(left_px: u32, top_px: u32) -> headless_chrome::protocol::cdp::DOM::Node {
    let style = format!("position:absolute;left:{}px;top:{}px", left_px, top_px);
    let div = make_element_node(
        4,
        "div",
        vec![("style", &style)],
        vec![make_text_node(5, "x")],
    );
    let body = make_element_node(3, "body", vec![], vec![div]);
    let html = make_element_node(2, "html", vec![], vec![body]);
    make_document_node(vec![html])
}

/// Helper: extract the stable_key of the positioned div (html > body > div).
fn div_key(
    dom: &headless_chrome::protocol::cdp::DOM::Node,
    viewport: ViewportDimensions,
) -> String {
    let root = normalize_dom_with_viewport(LoadProfile::Minimal, dom, viewport).unwrap();
    root.children[0] // html
        .children[0] // body
        .children[0] // div
        .stable_key
        .clone()
        .expect("positioned div must have a stable_key")
}

// ---------------------------------------------------------------------------
// Core regression: hardcoded 800×600 misassigns quadrant on large viewport
// ---------------------------------------------------------------------------

/// BUG REGRESSION: an element at (450, 350) on a 1920×1080 display belongs to
/// the top-left quadrant (x=450 < 960, y=350 < 540).
///
/// Before the fix, `normalize_dom` used the hardcoded 800×600 boundary, placing the
/// element in bottom-right (x=450 > 400, y=350 > 300) — an incorrect assignment.
/// After the fix, passing the actual 1920×1080 dimensions produces the correct
/// top-left stable_key.
#[test]
fn hardcoded_viewport_misassigns_quadrant_on_large_display() {
    let dom = positioned_doc(450, 350);

    // Pre-fix path: normalize_dom uses hardcoded 800×600.
    let key_hardcoded = {
        let root = normalize_dom(LoadProfile::Minimal, &dom).unwrap();
        root.children[0].children[0].children[0]
            .stable_key
            .clone()
            .expect("must have stable_key")
    };

    // Post-fix path: pass actual 1920×1080.
    let key_actual = div_key(
        &dom,
        ViewportDimensions {
            width: 1920.0,
            height: 1080.0,
        },
    );

    assert_ne!(
        key_hardcoded, key_actual,
        "BUG REGRESSION (ISSUE-03): stable_key for (450,350) must differ between \
         hardcoded 800×600 and actual 1920×1080 viewport — quadrant is wrong when \
         dimensions are hardcoded"
    );
}

/// The same element at (450, 350) in a 375×812 mobile viewport is in the top-right
/// quadrant (x=450 > 187.5 → right, y=350 < 406 → top).
/// Confirm that the mobile key also differs from the hardcoded-800×600 key.
#[test]
fn hardcoded_viewport_misassigns_quadrant_on_mobile_display() {
    let dom = positioned_doc(450, 350);

    let key_hardcoded = {
        let root = normalize_dom(LoadProfile::Minimal, &dom).unwrap();
        root.children[0].children[0].children[0]
            .stable_key
            .clone()
            .expect("must have stable_key")
    };

    // Mobile: x=450 > 187.5 → right, y=350 < 406 → top ⇒ top-right
    let key_mobile = div_key(
        &dom,
        ViewportDimensions {
            width: 375.0,
            height: 812.0,
        },
    );

    assert_ne!(
        key_hardcoded, key_mobile,
        "BUG REGRESSION (ISSUE-03): stable_key for (450,350) must differ between \
         hardcoded 800×600 and actual 375×812 mobile viewport"
    );
}

// ---------------------------------------------------------------------------
// normalize_dom_with_viewport API: actual dimensions produce correct quadrant
// ---------------------------------------------------------------------------

/// An element at (450, 350) with a 1920×1080 viewport lands in top-left.
/// An element at (450, 350) with 800×600 lands in bottom-right.
/// The two stable_keys must differ, and neither should match the 800×600 default.
#[test]
fn actual_viewport_dimensions_produce_correct_quadrant() {
    let dom = positioned_doc(450, 350);

    // 800×600: x=450>400 → right, y=350>300 → bottom ⇒ bottom-right
    let key_800 = div_key(
        &dom,
        ViewportDimensions {
            width: 800.0,
            height: 600.0,
        },
    );
    // 1920×1080: x=450<960 → left, y=350<540 → top ⇒ top-left
    let key_1920 = div_key(
        &dom,
        ViewportDimensions {
            width: 1920.0,
            height: 1080.0,
        },
    );

    assert_ne!(
        key_800, key_1920,
        "different viewport dimensions must produce different quadrant → different stable_key"
    );
}

/// When the element is well inside one quadrant at all viewport sizes, the stable_key
/// must remain stable across viewport changes (quadrant doesn't change).
///
/// Element at (50, 50) is top-left for any viewport wider than 100px and taller than 100px.
#[test]
fn stable_element_has_same_quadrant_across_viewports() {
    let dom = positioned_doc(50, 50);

    let key_800 = div_key(
        &dom,
        ViewportDimensions {
            width: 800.0,
            height: 600.0,
        },
    );
    let key_1920 = div_key(
        &dom,
        ViewportDimensions {
            width: 1920.0,
            height: 1080.0,
        },
    );
    let key_mobile = div_key(
        &dom,
        ViewportDimensions {
            width: 375.0,
            height: 812.0,
        },
    );

    assert_eq!(
        key_800, key_1920,
        "top-left element must have same stable_key at 800×600 and 1920×1080"
    );
    assert_eq!(
        key_800, key_mobile,
        "top-left element must have same stable_key at 800×600 and 375×812"
    );
}

/// Calling `normalize_dom_with_viewport` with explicit 800×600 must produce the
/// same result as `normalize_dom` (backward-compatibility guarantee).
#[test]
fn explicit_800x600_matches_normalize_dom_default() {
    let dom = positioned_doc(450, 350);

    let key_default = {
        let root = normalize_dom(LoadProfile::Minimal, &dom).unwrap();
        root.children[0].children[0].children[0]
            .stable_key
            .clone()
            .expect("must have stable_key")
    };

    let key_explicit = div_key(
        &dom,
        ViewportDimensions {
            width: 800.0,
            height: 600.0,
        },
    );

    assert_eq!(
        key_default, key_explicit,
        "normalize_dom_with_viewport(800×600) must be backward-compatible with normalize_dom"
    );
}

// ---------------------------------------------------------------------------
// PageSession integration: actual browser viewport is used for state capture
// (skipped when Chrome is unavailable)
// ---------------------------------------------------------------------------

/// Smoke test: a PageSession configured with a 1280×720 window must produce a different
/// stable_key for a straddling element than one configured with 375×812.
///
/// This validates that `PageSession::capture_semantic_state` calls
/// `get_viewport_dimensions()` and forwards the result to `normalize_dom_with_viewport`.
#[test]
fn page_session_uses_actual_viewport_for_stable_key() -> anyhow::Result<()> {
    if !core_runtime::chrome_available() {
        return Ok(());
    }

    // Button at left:300px, top:200px.
    //   1280×720 → 300/1280 = 0.234 < 0.5 → left → Top_Left
    //   375×812  → 300/375  = 0.800 ≥ 0.5 → right → Top_Right
    let fixture_html = r#"<!DOCTYPE html><html><body style="margin:0;padding:0;">
<button id="cta" style="position:absolute;left:300px;top:200px;">Click</button>
</body></html>"#;
    let url = format!("data:text/html,{}", urlencoding::encode(fixture_html));

    let desktop = core_runtime::BrowserClient::new_with_window_size(1280, 720)?;
    let desktop_page = desktop.new_page()?;
    desktop_page.navigate(&url)?;
    let desktop_state = desktop_page.capture_semantic_state(LoadProfile::Interactive)?;

    let mobile = core_runtime::BrowserClient::new_with_window_size(375, 812)?;
    let mobile_page = mobile.new_page()?;
    mobile_page.navigate(&url)?;
    let mobile_state = mobile_page.capture_semantic_state(LoadProfile::Interactive)?;

    fn find_button(state: &core_runtime::sre::SemanticState) -> Option<String> {
        fn search(node: &core_runtime::sre::SemanticNode) -> Option<String> {
            if node.role == "button" {
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

    let desktop_key = find_button(&desktop_state).expect("desktop must find the button");
    let mobile_key = find_button(&mobile_state).expect("mobile must find the button");

    assert_ne!(
        desktop_key, mobile_key,
        "BUG REGRESSION (ISSUE-03): PageSession must use actual viewport for stable_key — \
         desktop (1280×720) and mobile (375×812) must produce different quadrant for \
         a straddling element\n\
         desktop: {desktop_key}\n\
         mobile:  {mobile_key}"
    );

    Ok(())
}
