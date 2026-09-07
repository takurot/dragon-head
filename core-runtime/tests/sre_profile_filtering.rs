//! Tests for per-profile resource control (block/allow).
//! PLAN.md PR-02 テストタスク: Profile別のリソース制御（ブロック/許可）テスト

use core_runtime::sre::{normalize_dom, LoadProfile, SemanticState};
use core_runtime::BrowserClient;

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
    if test_bench_support::should_skip_browser_tests() {
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
        !roles.iter().any(|r| r == "script"),
        "Minimal must block <script>"
    );
    assert!(
        !roles.iter().any(|r| r == "style"),
        "Minimal must block <style>"
    );
    assert!(
        !roles.iter().any(|r| r == "img"),
        "Minimal must block <img>"
    );
    assert!(
        !roles.iter().any(|r| r == "video"),
        "Minimal must block <video>"
    );
    assert!(
        !roles.iter().any(|r| r == "svg"),
        "Minimal must block <svg>"
    );
    assert!(
        !roles.iter().any(|r| r == "iframe"),
        "Minimal must block <iframe>"
    );
    assert!(
        !roles.iter().any(|r| r == "canvas"),
        "Minimal must block <canvas>"
    );

    // Minimal MUST keep: h1, p, text
    assert!(roles.iter().any(|r| r == "h1"), "Minimal must keep <h1>");
    assert!(roles.iter().any(|r| r == "p"), "Minimal must keep <p>");

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
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let html = make_test_html();
    let (_client, page) = setup_page(&html)?;
    let root_node = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Visual, &root_node)?;
    let state = SemanticState::new(sem, LoadProfile::Visual);
    let roles = get_all_roles(&state);

    // Visual MUST allow: img, svg (for SoM generation)
    assert!(roles.iter().any(|r| r == "img"), "Visual must allow <img>");
    assert!(roles.iter().any(|r| r == "svg"), "Visual must allow <svg>");

    // Visual MUST block: script, style, video, iframe, canvas
    assert!(
        !roles.iter().any(|r| r == "script"),
        "Visual must block <script>"
    );
    assert!(
        !roles.iter().any(|r| r == "style"),
        "Visual must block <style>"
    );
    assert!(
        !roles.iter().any(|r| r == "video"),
        "Visual must block <video>"
    );
    assert!(
        !roles.iter().any(|r| r == "iframe"),
        "Visual must block <iframe>"
    );
    assert!(
        !roles.iter().any(|r| r == "canvas"),
        "Visual must block <canvas>"
    );

    // Text content must remain
    assert!(roles.iter().any(|r| r == "h1"), "Visual must keep <h1>");

    // role="presentation" must be excluded, matching Minimal and Interactive
    // (issue #282 — this assertion was missing from the Visual profile test).
    let json = serde_json::to_string(state.root())?;
    assert!(
        !json.contains("Ad Banner"),
        "Visual must exclude role=presentation content"
    );

    Ok(())
}

// ─── Interactive Profile ────────────────────────────────────────────

#[test]
fn test_interactive_allows_js_and_images() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
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
        roles.iter().any(|r| r == "script"),
        "Interactive must allow <script>"
    );
    assert!(
        roles.iter().any(|r| r == "img"),
        "Interactive must allow <img>"
    );
    assert!(
        roles.iter().any(|r| r == "svg"),
        "Interactive must allow <svg>"
    );
    assert!(
        roles.iter().any(|r| r == "style"),
        "Interactive must allow <style>"
    );
    assert!(
        roles.iter().any(|r| r == "video"),
        "Interactive must allow <video>"
    );
    assert!(
        roles.iter().any(|r| r == "iframe"),
        "Interactive must allow <iframe>"
    );
    assert!(
        roles.iter().any(|r| r == "canvas"),
        "Interactive must allow <canvas>"
    );

    // Text content must remain
    assert!(
        roles.iter().any(|r| r == "h1"),
        "Interactive must keep <h1>"
    );
    assert!(roles.iter().any(|r| r == "p"), "Interactive must keep <p>");

    // role="presentation" is still excluded (ads)
    let json = serde_json::to_string(state.root())?;
    assert!(
        !json.contains("Ad Banner"),
        "Interactive must exclude role=presentation content"
    );

    Ok(())
}
