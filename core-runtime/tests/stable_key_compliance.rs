
use core_runtime::sre::{normalize_dom, LoadProfile, SemanticState};
use core_runtime::BrowserClient;

#[test]
fn test_stable_key_format_compliance() -> anyhow::Result<()> {
    if !core_runtime::chrome_available() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    let url = "data:text/html,<html><body><div id='test'></div></body></html>";
    page.navigate(url)?;

    let node = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Minimal, &node)?;
    let state = SemanticState::new(sem, LoadProfile::Minimal);

    let keys = extract_keys(&state);
    let re = regex::Regex::new(r"^[a-f0-9]{64}$").unwrap();

    for key in keys {
        assert!(
            re.is_match(&key),
            "Key '{}' does not match SHA-256 hex format",
            key
        );
    }
    Ok(())
}

#[test]
fn test_ambiguous_flag_on_collision() -> anyhow::Result<()> {
    if !core_runtime::chrome_available() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    // Create collision scenario: two identical buttons
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

    // Collect (key, ambiguous) tuples
    let nodes = flatten_nodes(state.root());

    let buttons: Vec<_> = nodes.iter().filter(|n| n.role == "button").collect();

    assert_eq!(buttons.len(), 2, "Should find 2 buttons");

    let btn1 = buttons[0];
    let btn2 = buttons[1];

    // Check if ambiguous flag is set correctly
    // Depending on implementation, one might be false (first) and other true (second/collision)
    // OR both true if we track global ambiguity.
    // The implementation of StableKeyGenerator currently returns (base_hash, false) on first hit,
    // and (re-hashed, true) on second hit.
    // However, if the first one is later found to start a chain of collisions, strictly speaking it is also part of the ambiguous set?
    // But for "stable key" purposes, the first one owns the premier key.

    assert!(!btn1.ambiguous, "First occurrence should not be ambiguous");
    assert!(
        btn2.ambiguous,
        "Second occurrence (collision) should be ambiguous"
    );

    assert_ne!(btn1.stable_key, btn2.stable_key, "Keys must differ");

    Ok(())
}

fn flatten_nodes(
    node: &core_runtime::sre::state::SemanticNode,
) -> Vec<&core_runtime::sre::state::SemanticNode> {
    let mut list = Vec::new();
    list.push(node);
    for child in &node.children {
        list.extend(flatten_nodes(child));
    }
    list
}

fn extract_keys(state: &SemanticState) -> Vec<String> {
    flatten_nodes(state.root())
        .iter()
        .filter_map(|n| n.stable_key.clone())
        .collect()
}
