use core_runtime::sre::{normalize_dom, LoadProfile, SemanticNode, SemanticState};
use core_runtime::BrowserClient;

fn should_skip() -> bool {
    std::env::var("CI").is_ok() && std::env::var("CHROME_INSTALLED").is_err()
}

#[test]
fn test_stable_key_tracks_quadrant_not_dom_order() -> anyhow::Result<()> {
    if should_skip() {
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
fn test_alias_output_and_stable_key_index_consistency() -> anyhow::Result<()> {
    if should_skip() {
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
