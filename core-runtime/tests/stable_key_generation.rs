use core_runtime::sre::{normalize_dom, LoadProfile, SemanticState};
use core_runtime::BrowserClient;

#[test]
fn test_stable_key_determinism() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <button id="btn1">Click Me</button>
                <div class="container">
                    <span>Text Content</span>
                </div>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));

    // Run 1
    page.navigate(&url)?;
    let node1 = page.get_document_node()?;
    let sem1 = normalize_dom(LoadProfile::Minimal, &node1)?;
    let state1 = SemanticState::new(sem1, LoadProfile::Minimal);

    // Run 2 (Reload)
    page.navigate(&url)?;
    let node2 = page.get_document_node()?;
    let sem2 = normalize_dom(LoadProfile::Minimal, &node2)?;
    let state2 = SemanticState::new(sem2, LoadProfile::Minimal);

    // Extract keys
    let keys1 = extract_keys(&state1);
    let keys2 = extract_keys(&state2);

    assert_eq!(
        keys1, keys2,
        "Stable keys must be deterministic across reloads"
    );
    assert!(!keys1.is_empty(), "Should generate keys");

    // Verify format (SHA-256 hex)
    for key in keys1 {
        // We might append suffix for collision, so check prefix length or existence
        assert!(key.len() >= 64, "Key should be at least SHA-256 length");
    }

    Ok(())
}

#[test]
fn test_stable_key_collision_handling() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    // Two identical buttons
    let html = r#"
        <html>
            <body>
                <button>Submit</button>
                <button>Submit</button>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let node = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Minimal, &node)?;
    let state = SemanticState::new(sem, LoadProfile::Minimal);

    let keys = extract_keys(&state);
    // Verify that all generated keys are unique (collision handling works)
    // We expect multiple keys (html, body, button1, button2), and they must all be distinct.
    let unique_keys: std::collections::HashSet<_> = keys.iter().collect();
    assert_eq!(
        keys.len(),
        unique_keys.len(),
        "All generated keys must be unique even for identical elements"
    );

    // Check that we have at least 2 keys (buttons)
    assert!(keys.len() >= 2, "Should parse at least the buttons");

    Ok(())
}

fn extract_keys(state: &SemanticState) -> Vec<String> {
    let mut keys = Vec::new();
    collect_keys(state.root(), &mut keys);
    keys
}

fn collect_keys(node: &core_runtime::sre::state::SemanticNode, keys: &mut Vec<String>) {
    // Check the struct field, not attributes map
    if let Some(key) = &node.stable_key {
        keys.push(key.clone());
    }
    for child in &node.children {
        collect_keys(child, keys);
    }
}
