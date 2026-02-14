use core_runtime::sre::{normalize_dom, LoadProfile, SemanticState};
use core_runtime::BrowserClient;

fn should_skip() -> bool {
    std::env::var("CI").is_ok() && std::env::var("CHROME_INSTALLED").is_err()
}

#[test]
fn test_repro_unstable_keys_on_sibling_insertion() -> anyhow::Result<()> {
    if should_skip() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    // Initial State: Container with a span
    let html_1 = r#"
        <html>
            <body>
                <div id="container">
                    <span id="target">Target Content</span>
                </div>
            </body>
        </html>
    "#;
    let url_1 = format!("data:text/html,{}", urlencoding::encode(html_1));
    page.navigate(&url_1)?;
    let node1 = page.get_document_node()?;
    let sem1 = normalize_dom(LoadProfile::Minimal, &node1)?;
    let state1 = SemanticState::new(sem1, LoadProfile::Minimal);
    
    println!("--- Run 1 Tree ---");
    print_tree(state1.root(), 0);
    println!("------------------");

    let all_keys = extract_keys(&state1);
    println!("Run 1 Keys: {:?}", all_keys);

    // normalizer uses ID as label hint if available
    let target_key_1 = find_key_by_role_and_label(&state1, "span", "target")
        .expect("Target node should exist in run 1");

    // Modified State: Insert a button BEFORE the container (or inside? Spec says "parent_path").
    // If we insert a sibling before the `div#container`, the `div`'s index changes.
    // If the proper implementation uses structural path WITHOUT index, the `div`'s key shouldn't change (as it's unique by ID/Structure?), 
    // OR at least the `span` inside it shouldn't change just because the parent's index changed?
    // Actually, if parent's stable key changes, child's stable key probably changes (dom_signature dependency).
    // Let's try inserting a sibling to the TARGET span.
    
    let html_2 = r#"
        <html>
            <body>
                <div id="container">
                    <button>Inserted Sibling</button>
                    <span id="target">Target Content</span>
                </div>
            </body>
        </html>
    "#;
    let url_2 = format!("data:text/html,{}", urlencoding::encode(html_2));
    page.navigate(&url_2)?;
    let node2 = page.get_document_node()?;
    let sem2 = normalize_dom(LoadProfile::Minimal, &node2)?;
    let state2 = SemanticState::new(sem2, LoadProfile::Minimal);

    let all_keys_2 = extract_keys(&state2);
    println!("Run 2 Keys: {:?}", all_keys_2);

    let target_key_2 = find_key_by_role_and_label(&state2, "span", "target")
        .expect("Target node should exist in run 2");

    assert_eq!(
        target_key_1, target_key_2,
        "Stable key for unique target should NOT change when a sibling is inserted"
    );

    Ok(())
}

fn find_key_by_role_and_label(state: &SemanticState, role: &str, label: &str) -> Option<String> {
    find_recursive(state.root(), role, label)
}

fn find_recursive(node: &core_runtime::sre::state::SemanticNode, role: &str, label: &str) -> Option<String> {
    if node.role == role && node.label.as_deref() == Some(label) {
        return node.stable_key.clone();
    }
    for child in &node.children {
        if let Some(k) = find_recursive(child, role, label) {
            return Some(k);
        }
    }
    None
}

fn print_tree(node: &core_runtime::sre::state::SemanticNode, depth: usize) {
    let indent = "  ".repeat(depth);
    println!("{}{:?} label={:?} key={:?}", indent, node.role, node.label, node.stable_key);
    for child in &node.children {
        print_tree(child, depth + 1);
    }
}

fn extract_keys(state: &SemanticState) -> Vec<String> {
    flatten_nodes(state.root()).iter()
        .filter_map(|n| n.stable_key.clone())
        .collect()
}

fn flatten_nodes(node: &core_runtime::sre::state::SemanticNode) -> Vec<&core_runtime::sre::state::SemanticNode> {
    let mut list = Vec::new();
    list.push(node);
    for child in &node.children {
        list.extend(flatten_nodes(child));
    }
    list
}
