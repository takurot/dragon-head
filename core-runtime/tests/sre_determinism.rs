use core_runtime::should_skip_browser_tests;
use core_runtime::{
    sre::{normalize_dom, LoadProfile, SemanticState},
    BrowserClient,
};

/// Skip test if Chrome is not available in CI

#[test]
fn test_sre_determinism_same_input() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html_content = r#"
        <html>
            <body>
                <div class="dynamic-12345 constant">Hello</div>
                <div role="presentation">Ad</div>
                <script>console.log('block me');</script>
            </body>
        </html>
    "#;

    let url = format!("data:text/html,{}", urlencoding::encode(html_content));
    let profile = LoadProfile::Minimal;

    // Run 1
    page.navigate(&url)?;
    let root_node1 = page.get_document_node()?;
    let sem_node1 = normalize_dom(profile, &root_node1)?;
    let state1 = SemanticState::new(sem_node1, profile);

    // Run 2 (reload)
    page.navigate(&url)?;
    let root_node2 = page.get_document_node()?;
    let sem_node2 = normalize_dom(profile, &root_node2)?;
    let state2 = SemanticState::new(sem_node2, profile);

    // Assert determinism
    assert_eq!(
        state1.state_hash(),
        state2.state_hash(),
        "Hashes must be identical for same input"
    );
    assert!(!state1.state_hash().is_empty(), "Hash must be non-empty");

    // Check filtering
    let json = serde_json::to_string(state1.root())?;
    assert!(
        !json.contains("\"role\":\"script\""),
        "Script tag should be filtered in Minimal"
    );
    assert!(
        !json.contains("Ad"),
        "Content inside role=presentation should be filtered"
    );
    assert!(json.contains("Hello"), "Main content should be preserved");

    Ok(())
}

/// Review finding #5: Verify that dynamic class token changes do NOT affect state_hash.
/// This is the core determinism requirement of SPEC SRE-01.
#[test]
fn test_sre_determinism_dynamic_class_variance() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let profile = LoadProfile::Minimal;

    // HTML with dynamic-looking class "sc-abc12345"
    let html_a = r#"
        <html>
            <body>
                <div class="btn sc-abc12345 primary">Click me</div>
                <p class="css-99zz88yy">Paragraph</p>
            </body>
        </html>
    "#;

    // Same structure but DIFFERENT dynamic class tokens
    let html_b = r#"
        <html>
            <body>
                <div class="btn sc-xyz99999 primary">Click me</div>
                <p class="css-11aa22bb">Paragraph</p>
            </body>
        </html>
    "#;

    let page_a = client.new_page()?;
    let url_a = format!("data:text/html,{}", urlencoding::encode(html_a));
    page_a.navigate(&url_a)?;
    let node_a = page_a.get_document_node()?;
    let sem_a = normalize_dom(profile, &node_a)?;
    let state_a = SemanticState::new(sem_a, profile);

    let page_b = client.new_page()?;
    let url_b = format!("data:text/html,{}", urlencoding::encode(html_b));
    page_b.navigate(&url_b)?;
    let node_b = page_b.get_document_node()?;
    let sem_b = normalize_dom(profile, &node_b)?;
    let state_b = SemanticState::new(sem_b, profile);

    // The ONLY difference is the dynamic class names, which should be stripped.
    // Therefore the hashes MUST be equal.
    assert_eq!(
        state_a.state_hash(),
        state_b.state_hash(),
        "state_hash must be stable across dynamic class changes"
    );

    // Verify that semantic classes ("btn", "primary") survived
    let json_a = serde_json::to_string(state_a.root())?;
    assert!(
        json_a.contains("btn"),
        "Semantic class 'btn' should be preserved"
    );
    assert!(
        json_a.contains("primary"),
        "Semantic class 'primary' should be preserved"
    );
    // Dynamic class should NOT be in the output
    assert!(
        !json_a.contains("sc-abc12345"),
        "Dynamic class should be stripped"
    );

    Ok(())
}
