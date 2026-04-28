//! Tests for per-profile resource control (block/allow).
//! PLAN.md PR-02 テストタスク: Profile別のリソース制御（ブロック/許可）テスト

use core_runtime::sre::{normalize_dom, LoadProfile, SemanticState};
use core_runtime::BrowserClient;
use core_runtime::should_skip_browser_tests;

fn make_test_html() -> String {
    r#"
    <html>
        <body>
            <h1>Hello World</h1>
            <img src="test.png" alt="photo" />
            <video src="movie.mp4"></video>
            <script>console.log('app');</script>
            <style>.x { color: red; }</style>
            <div role="presentation">Ad Banner</div>
            <iframe src="https://ads.example.com"></iframe>
            <svg><circle cx="10" cy="10" r="5"/></svg>
            <canvas id="chart"></canvas>
            <p>Main content paragraph</p>
        </body>
    </html>
    "#
    .to_string()
}

fn setup_page(html: &str) -> anyhow::Result<(BrowserClient, core_runtime::PageSession)> {
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;
    Ok((client, page))
}

fn collect_roles(node: &core_runtime::sre::state::SemanticNode, roles: &mut Vec<String>) {
    roles.push(node.role.clone());
    for child in &node.children {
        collect_roles(child, roles);
    }
}

fn get_all_roles(state: &core_runtime::sre::SemanticState) -> Vec<String> {
    let mut roles = Vec::new();
    collect_roles(state.root(), &mut roles);
    roles
}

// ─── Minimal Profile ────────────────────────────────────────────────

#[test]
fn test_minimal_blocks_all_media_and_js() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let html = make_test_html();
    let (_client, page) = setup_page(&html)?;
    let root_node = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Minimal, &root_node)?;
    let state = SemanticState::new(sem, LoadProfile::Minimal);
    let roles = get_all_roles(&state);

    // Minimal MUST block: script, style, img, video, svg, iframe, canvas
    assert!(
        !roles.contains(&"script".to_string()),
        "Minimal must block <script>"
    );
    assert!(
        !roles.contains(&"style".to_string()),
        "Minimal must block <style>"
    );
    assert!(
        !roles.contains(&"img".to_string()),
        "Minimal must block <img>"
    );
    assert!(
        !roles.contains(&"video".to_string()),
        "Minimal must block <video>"
    );
    assert!(
        !roles.contains(&"svg".to_string()),
        "Minimal must block <svg>"
    );
    assert!(
        !roles.contains(&"iframe".to_string()),
        "Minimal must block <iframe>"
    );
    assert!(
        !roles.contains(&"canvas".to_string()),
        "Minimal must block <canvas>"
    );

    // Minimal MUST keep: h1, p, text
    assert!(roles.contains(&"h1".to_string()), "Minimal must keep <h1>");
    assert!(roles.contains(&"p".to_string()), "Minimal must keep <p>");

    // role="presentation" must be excluded
    let json = serde_json::to_string(state.root())?;
    assert!(
        !json.contains("Ad Banner"),
        "Minimal must exclude role=presentation content"
    );

    Ok(())
}

// ─── Visual Profile ─────────────────────────────────────────────────

#[test]
fn test_visual_allows_images_blocks_js() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let html = make_test_html();
    let (_client, page) = setup_page(&html)?;
    let root_node = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Visual, &root_node)?;
    let state = SemanticState::new(sem, LoadProfile::Visual);
    let roles = get_all_roles(&state);

    // Visual MUST allow: img, svg (for SoM generation)
    assert!(
        roles.contains(&"img".to_string()),
        "Visual must allow <img>"
    );
    assert!(
        roles.contains(&"svg".to_string()),
        "Visual must allow <svg>"
    );

    // Visual MUST block: script, style, video, iframe, canvas
    assert!(
        !roles.contains(&"script".to_string()),
        "Visual must block <script>"
    );
    assert!(
        !roles.contains(&"style".to_string()),
        "Visual must block <style>"
    );
    assert!(
        !roles.contains(&"video".to_string()),
        "Visual must block <video>"
    );
    assert!(
        !roles.contains(&"iframe".to_string()),
        "Visual must block <iframe>"
    );
    assert!(
        !roles.contains(&"canvas".to_string()),
        "Visual must block <canvas>"
    );

    // Text content must remain
    assert!(roles.contains(&"h1".to_string()), "Visual must keep <h1>");

    Ok(())
}

// ─── Interactive Profile ────────────────────────────────────────────

#[test]
fn test_interactive_allows_js_and_images() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let html = make_test_html();
    let (_client, page) = setup_page(&html)?;
    let root_node = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Interactive, &root_node)?;
    let state = SemanticState::new(sem, LoadProfile::Interactive);
    let roles = get_all_roles(&state);

    // Interactive MUST allow: script, img, svg, style, video, iframe, canvas
    assert!(
        roles.contains(&"script".to_string()),
        "Interactive must allow <script>"
    );
    assert!(
        roles.contains(&"img".to_string()),
        "Interactive must allow <img>"
    );
    assert!(
        roles.contains(&"style".to_string()),
        "Interactive must allow <style>"
    );

    // Text content must remain
    assert!(
        roles.contains(&"h1".to_string()),
        "Interactive must keep <h1>"
    );
    assert!(
        roles.contains(&"p".to_string()),
        "Interactive must keep <p>"
    );

    // role="presentation" is still excluded (ads)
    let json = serde_json::to_string(state.root())?;
    assert!(
        !json.contains("Ad Banner"),
        "Interactive must exclude role=presentation content"
    );

    Ok(())
}
