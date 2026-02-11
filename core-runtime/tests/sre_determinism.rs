use core_runtime::{
    sre::{normalize_dom, LoadProfile, SemanticState},
    BrowserClient,
};

#[test]
fn test_sre_determinism() -> anyhow::Result<()> {
    if std::env::var("CI").is_ok() {
        // Simple check to avoid running if chrome is missing in basic CI
        // Real CI sets CHROME_INSTALLED.
        if std::env::var("CHROME_INSTALLED").is_err() {
            return Ok(());
        }
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

    // Use data URI
    let url = format!("data:text/html,{}", urlencoding::encode(html_content));
    page.navigate(&url)?;

    let profile = LoadProfile::Minimal;
    let root_node = page.get_document_node()?;

    // Run 1
    let sem_node1 = normalize_dom(profile, &root_node)?;
    let state1 = SemanticState::new(sem_node1);

    // Run 2 (reload to verify determinism logic)
    page.navigate(&url)?;
    let root_node2 = page.get_document_node()?;
    let sem_node2 = normalize_dom(profile, &root_node2)?;
    let state2 = SemanticState::new(sem_node2);

    // Assert determinism
    assert_eq!(
        state1.state_hash, state2.state_hash,
        "Hashes must be identical for same input"
    );
    assert_ne!(
        state1.state_hash, "pending_hash",
        "Hash must be implemented"
    );

    // Check filtering
    let json = serde_json::to_string(&state1.root)?;
    // "script" tag should be filtered by Minimal profile
    // Note: normalize_dom lowercases tag names and uses them as role
    assert!(
        !json.contains("\"role\":\"script\""),
        "Script tag should be filtered"
    );
    // "Ad" text should be filtered because parent has role=presentation?
    // normalize_dom recursion: if filtered, returns None, so children are not processed.
    // Logic: if role=presentation, traverse_node returns Ok(None). Correct.
    assert!(
        !json.contains("Ad"),
        "Content inside presentation role should be filtered"
    );
    assert!(json.contains("Hello"), "Main content should be preserved");

    Ok(())
}
