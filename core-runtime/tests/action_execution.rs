use core_runtime::should_skip_browser_tests;
use core_runtime::{
    sre::{normalize_dom, LoadProfile, SemanticState},
    BrowserClient,
};

#[test]
fn test_action_execution_basic() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <button id="btn" onclick="document.body.style.backgroundColor = 'red'">Click Me</button>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let root = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Minimal, &root)?;
    let state = SemanticState::new(sem, LoadProfile::Minimal);

    // Find the button's backend_node_id and stable_key
    let (btn_id, btn_key) = find_button_info(state.root()).expect("Button not found");

    // Execute Click action
    // Note: act method is not implemented yet on PageSession
    page.act(Some(btn_id), Some(&btn_key), "click", None)?;

    // Verify effect (background color changed)
    let _summary = page.get_document_node()?; // Re-fetch to check style?
                                              // Or evaluate JS
    let bg_color = page
        .evaluate_script("document.body.style.backgroundColor")?
        .value
        .unwrap();
    assert_eq!(
        bg_color.as_str().unwrap(),
        "red",
        "Button click should change background color"
    );

    Ok(())
}

#[test]
fn test_action_execution_fallback() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    // Page that replaces the button on click (simulation of hydration/re-render)
    // But we want to simulate STALE ID.
    // So:
    // 1. Load page. 2. Get State. 3. Mutate DOM (replace button).
    // 4. Try to click using OLD ID and OLD Key.
    // 5. Fallback should find NEW button by Key and click it.

    let html = r#"
        <html>
            <body>
                <div id="container">
                    <button id="btn" onclick="document.body.dataset.clicked = 'true'">Target</button>
                </div>
                <script>
                    function replaceButton() {
                        const container = document.getElementById('container');
                        container.innerHTML = '<button id="btn" onclick="document.body.dataset.clicked = \'true\'">Target</button>';
                    }
                </script>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let root = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Minimal, &root)?;
    let state = SemanticState::new(sem, LoadProfile::Minimal);

    let (old_id, key) = find_button_info(state.root()).expect("Button not found");

    // Trigger mutation to invalidate old_id
    page.evaluate_script("replaceButton()")?;

    // Use a guaranteed stale ID to force fallback path deterministically.
    let stale_id = old_id + 1_000_000;
    page.act(Some(stale_id), Some(&key), "click", None)?;

    // Verify effect
    let clicked = page.evaluate_script("document.body.dataset.clicked")?.value;
    assert_eq!(
        clicked.unwrap().as_str().unwrap(),
        "true",
        "Fallback click should work"
    );

    let logs = page.action_logs()?;
    assert!(
        logs.iter().any(|entry| {
            entry.level == "warning"
                && entry.code == "stable_key_fallback_recovered"
                && entry.action == "click"
                && entry.stable_key.as_deref() == Some(key.as_str())
        }),
        "Expected structured warning log for stable_key fallback recovery"
    );

    Ok(())
}

fn find_button_info(node: &core_runtime::sre::state::SemanticNode) -> Option<(i64, String)> {
    if node.role == "button" {
        return Some((
            node.backend_node_id,
            node.stable_key.clone().unwrap_or_default(),
        ));
    }
    for child in &node.children {
        if let Some(res) = find_button_info(child) {
            return Some(res);
        }
    }
    None
}

#[test]
fn test_action_execution_verify_required() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html><body><button id="btn">Gone</button></body></html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    // Use a completely fake ID and a fake key that won't be found
    let fake_id = 999999;
    let fake_key = "0000000000000000000000000000000000000000000000000000000000000000";

    // Action should fail with VerifyRequired
    let result = page.act(Some(fake_id), Some(fake_key), "click", None);

    assert!(result.is_err());
    let err = result.unwrap_err();

    // Check if error is ActionError::VerifyRequired
    if let Some(action_err) = err.downcast_ref::<core_runtime::error::ActionError>() {
        if matches!(action_err, core_runtime::error::ActionError::VerifyRequired) {
            let logs = page.action_logs()?;
            assert!(
                logs.iter().any(|entry| {
                    entry.level == "error"
                        && entry.code == "verify_required"
                        && entry.action == "click"
                }),
                "Expected structured error log for verify required path"
            );
            return Ok(());
        }
        panic!("Expected VerifyRequired, got ActionError variant: {action_err:?}");
    }

    panic!("Expected ActionError::VerifyRequired, got: {:?}", err);
}
