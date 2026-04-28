use core_runtime::sre::{normalize_dom, LoadProfile, SemanticNode, SemanticState};
use core_runtime::BrowserClient;
use core_runtime::should_skip_browser_tests;

#[test]
fn test_stable_key_tracks_quadrant_not_dom_order() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html_v1 = r#"
        <html>
            <body>
                <button data-quadrant="top_left">Checkout</button>
                <button data-quadrant="top_right">Checkout</button>
            </body>
        </html>
    "#;
    let url_v1 = format!("data:text/html,{}", urlencoding::encode(html_v1));
    page.navigate(&url_v1)?;

    let state_v1 = capture_state(&page)?;
    let top_left_v1 = find_button_key_by_quadrant(&state_v1, "top_left")
        .expect("top_left button should have a stable key");
    let top_right_v1 = find_button_key_by_quadrant(&state_v1, "top_right")
        .expect("top_right button should have a stable key");

    let html_v2 = r#"
        <html>
            <body>
                <button data-quadrant="top_right">Checkout</button>
                <button data-quadrant="top_left">Checkout</button>
            </body>
        </html>
    "#;
    let url_v2 = format!("data:text/html,{}", urlencoding::encode(html_v2));
    page.navigate(&url_v2)?;

    let state_v2 = capture_state(&page)?;
    let top_left_v2 = find_button_key_by_quadrant(&state_v2, "top_left")
        .expect("top_left button should have a stable key after re-render");
    let top_right_v2 = find_button_key_by_quadrant(&state_v2, "top_right")
        .expect("top_right button should have a stable key after re-render");

    assert_ne!(
        top_left_v1, top_right_v1,
        "Keys for different quadrants must differ"
    );
    assert_eq!(
        top_left_v1, top_left_v2,
        "top_left key must remain stable even when DOM order changes"
    );
    assert_eq!(
        top_right_v1, top_right_v2,
        "top_right key must remain stable even when DOM order changes"
    );

    Ok(())
}

#[test]
fn test_stable_key_tracks_quadrant_from_style_pixels() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html_v1 = r#"
        <html>
            <body>
                <button style="position:absolute; left:40px; top:40px;">Checkout</button>
                <button style="position:absolute; left:740px; top:40px;">Checkout</button>
            </body>
        </html>
    "#;
    let url_v1 = format!("data:text/html,{}", urlencoding::encode(html_v1));
    page.navigate(&url_v1)?;

    let state_v1 = capture_state(&page)?;
    let left_key_v1 = find_button_key_by_style_fragment(&state_v1, "left:40px")
        .expect("left button should have a stable key");
    let right_key_v1 = find_button_key_by_style_fragment(&state_v1, "left:740px")
        .expect("right button should have a stable key");

    let html_v2 = r#"
        <html>
            <body>
                <button style="position:absolute; left:740px; top:40px;">Checkout</button>
                <button style="position:absolute; left:40px; top:40px;">Checkout</button>
            </body>
        </html>
    "#;
    let url_v2 = format!("data:text/html,{}", urlencoding::encode(html_v2));
    page.navigate(&url_v2)?;

    let state_v2 = capture_state(&page)?;
    let left_key_v2 = find_button_key_by_style_fragment(&state_v2, "left:40px")
        .expect("left button should have a stable key after re-render");
    let right_key_v2 = find_button_key_by_style_fragment(&state_v2, "left:740px")
        .expect("right button should have a stable key after re-render");

    assert_ne!(
        left_key_v1, right_key_v1,
        "Keys for left/right pixel quadrants must differ"
    );
    assert_eq!(
        left_key_v1, left_key_v2,
        "left button key must remain stable even when DOM order changes"
    );
    assert_eq!(
        right_key_v1, right_key_v2,
        "right button key must remain stable even when DOM order changes"
    );

    Ok(())
}

#[test]
fn test_alias_output_and_stable_key_index_consistency() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <button aria-label="Purchase now">Buy</button>
                <input aria-label="Email Address" />
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let state = capture_state(&page)?;
    let fast = state.generate_fast_state();
    let button = fast
        .interactive_elements
        .iter()
        .find(|node| node.role == "button")
        .expect("button must exist in interactive elements");

    let expected_alias = button
        .alias
        .as_deref()
        .filter(|alias| !alias.is_empty())
        .expect("interactive button must expose non-empty alias")
        .to_string();

    let stable_key = button
        .stable_key
        .as_deref()
        .expect("button must expose stable_key")
        .to_string();
    let backend_node_id = button.backend_node_id;

    let indexed_count = page.refresh_stable_key_index(LoadProfile::Interactive)?;
    assert!(indexed_count > 0, "stable key index must be populated");

    let resolved_id = page.lookup_backend_node_id_by_stable_key(&stable_key);
    assert_eq!(
        resolved_id,
        Some(backend_node_id),
        "stable_key index lookup must resolve to the same node id as fast state"
    );
    assert_eq!(
        page.lookup_alias_by_stable_key(&stable_key),
        Some(expected_alias),
        "alias in stable_key index must match fast state output"
    );

    Ok(())
}

#[test]
fn test_stable_key_index_is_cleared_on_navigation() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html_a = r#"
        <html>
            <body>
                <button aria-label="Purchase now">Buy</button>
            </body>
        </html>
    "#;
    let url_a = format!("data:text/html,{}", urlencoding::encode(html_a));
    page.navigate(&url_a)?;

    let state_a = capture_state(&page)?;
    let stable_key = state_a
        .generate_fast_state()
        .interactive_elements
        .into_iter()
        .find(|node| node.role == "button")
        .and_then(|node| node.stable_key)
        .expect("button stable_key must exist");

    page.refresh_stable_key_index(LoadProfile::Interactive)?;
    assert!(
        page.lookup_backend_node_id_by_stable_key(&stable_key)
            .is_some(),
        "stable_key should be resolvable before navigation"
    );

    let html_b = r#"
        <html>
            <body>
                <p>different page</p>
            </body>
        </html>
    "#;
    let url_b = format!("data:text/html,{}", urlencoding::encode(html_b));
    page.navigate(&url_b)?;

    assert!(
        page.lookup_backend_node_id_by_stable_key(&stable_key)
            .is_none(),
        "stable_key index must not keep stale entries after navigation"
    );

    Ok(())
}

#[test]
fn test_minimal_capture_keeps_stable_key_lookup_available() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <button aria-label="Purchase now">Buy</button>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let state = page.capture_semantic_state(LoadProfile::Minimal)?;
    let button = state
        .generate_fast_state()
        .interactive_elements
        .into_iter()
        .find(|node| node.role == "button")
        .expect("button must exist in minimal capture");
    let stable_key = button
        .stable_key
        .clone()
        .expect("button stable_key must exist");
    let alias = button.alias.clone().expect("button alias must exist");

    assert_eq!(
        page.lookup_backend_node_id_by_stable_key(&stable_key),
        Some(button.backend_node_id),
        "minimal capture must keep the stable_key lookup index populated"
    );
    assert_eq!(
        page.lookup_alias_by_stable_key(&stable_key),
        Some(alias),
        "minimal capture must keep alias lookup aligned with the stable_key index"
    );

    Ok(())
}

fn capture_state(page: &core_runtime::PageSession) -> anyhow::Result<SemanticState> {
    let node = page.get_document_node()?;
    let root = normalize_dom(LoadProfile::Minimal, &node)?;
    Ok(SemanticState::new(root, LoadProfile::Minimal))
}

fn find_button_key_by_quadrant(state: &SemanticState, quadrant: &str) -> Option<String> {
    find_button_key_by_quadrant_recursive(state.root(), quadrant)
}

fn find_button_key_by_quadrant_recursive(node: &SemanticNode, quadrant: &str) -> Option<String> {
    if node.role == "button"
        && node
            .attributes
            .as_ref()
            .and_then(|attrs| attrs.get("data-quadrant"))
            .is_some_and(|value| value == quadrant)
    {
        return node.stable_key.clone();
    }

    for child in &node.children {
        if let Some(found) = find_button_key_by_quadrant_recursive(child, quadrant) {
            return Some(found);
        }
    }
    None
}

fn find_button_key_by_style_fragment(state: &SemanticState, needle: &str) -> Option<String> {
    find_button_key_by_style_fragment_recursive(state.root(), needle)
}

fn find_button_key_by_style_fragment_recursive(
    node: &SemanticNode,
    needle: &str,
) -> Option<String> {
    if node.role == "button"
        && node
            .attributes
            .as_ref()
            .and_then(|attrs| attrs.get("style"))
            .is_some_and(|value| value.contains(needle))
    {
        return node.stable_key.clone();
    }

    for child in &node.children {
        if let Some(found) = find_button_key_by_style_fragment_recursive(child, needle) {
            return Some(found);
        }
    }
    None
}
