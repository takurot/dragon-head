use core_runtime::should_skip_browser_tests;
use core_runtime::{
    sre::{normalize_dom, LoadProfile, SemanticState},
    BrowserClient,
};

#[test]
fn test_stable_key_self_heals_across_spa_rerenders() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <div id="app"></div>
                <script>
                    window.__clicks = [];
                    function render(route, step) {
                        const rows = Array.from({length: (step % 4) + 2}, (_, i) =>
                            `<li data-route="${route}" data-index="${i}">row-${route}-${i}</li>`
                        ).join("");
                        document.getElementById("app").innerHTML = `
                            <section data-route="${route}" data-step="${step}">
                                <aside>
                                    <button class="cta secondary" style="position:absolute; left:24px; top:24px;"
                                        onclick="window.__clicks.push('secondary:${route}')">
                                        Continue
                                    </button>
                                </aside>
                                <main>
                                    <h2>Checkout Flow</h2>
                                    <ul>${rows}</ul>
                                    <button class="cta primary" style="position:absolute; left:520px; top:24px;"
                                        onclick="window.__clicks.push('${route}')">
                                        Continue
                                    </button>
                                </main>
                            </section>
                        `;
                    }
                    render('catalog', 0);
                </script>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let mut current_target =
        capture_primary_button_info(&page)?.expect("primary CTA should exist on initial render");
    let stable_key = current_target.1.clone();
    let routes = [
        "shipping",
        "payment",
        "review",
        "confirmation",
        "upsell",
        "receipt",
    ];

    for (step, route) in routes.iter().enumerate() {
        let previous_target_id = current_target.0;
        page.evaluate_script(&format!("render('{route}', {});", step + 1))?;

        let refreshed_target = capture_primary_button_info(&page)?
            .expect("primary CTA should remain discoverable after rerender");
        assert_ne!(
            refreshed_target.0, previous_target_id,
            "SPA rerender for route {route} should replace the previous backend node id"
        );
        assert_eq!(
            refreshed_target.1, stable_key,
            "stable_key should stay fixed for the primary CTA across route {route}"
        );

        // Some Chromium backends keep detached backend_node_id values actionable for a short
        // time. Derive a guaranteed-invalid id from the retired node to force the fallback path
        // while still asserting that the real DOM id churned across the SPA transition.
        let forced_stale_id = previous_target_id + 1_000_000;
        page.act(Some(forced_stale_id), Some(&stable_key), "click", None)?;
        current_target = refreshed_target;
    }

    let click_log = page
        .evaluate_script("JSON.stringify(window.__clicks)")?
        .value
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .unwrap_or_default();
    assert_eq!(
        click_log,
        serde_json::to_string(&routes)?,
        "all route transitions should click the primary CTA through stale-id recovery"
    );

    let fallback_recoveries = page
        .action_logs()?
        .into_iter()
        .filter(|entry| entry.code == "stable_key_fallback_recovered")
        .count();
    assert!(
        fallback_recoveries >= routes.len(),
        "expected at least {} fallback recovery logs, got {fallback_recoveries}",
        routes.len()
    );

    Ok(())
}

fn capture_primary_button_info(
    page: &core_runtime::PageSession,
) -> anyhow::Result<Option<(i64, String)>> {
    let root = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Interactive, &root)?;
    let state = SemanticState::new(sem, LoadProfile::Interactive);
    Ok(find_node_info_by_class(state.root(), "primary"))
}

fn find_node_info_by_class(
    node: &core_runtime::sre::state::SemanticNode,
    class_name: &str,
) -> Option<(i64, String)> {
    let has_class = node
        .attributes
        .as_ref()
        .and_then(|attrs| attrs.get("class"))
        .is_some_and(|class| class.split_whitespace().any(|token| token == class_name));
    if has_class && node.role == "button" {
        return Some((node.backend_node_id, node.stable_key.clone()?));
    }

    for child in &node.children {
        if let Some(found) = find_node_info_by_class(child, class_name) {
            return Some(found);
        }
    }

    None
}
