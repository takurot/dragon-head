use core_runtime::{
    sre::{normalize_dom, LoadProfile, SemanticState},
    BrowserClient,
};

fn should_skip() -> bool {
    std::env::var("CI").is_ok() && std::env::var("CHROME_INSTALLED").is_err()
}

#[test]
fn test_stable_key_self_heals_across_spa_rerenders() -> anyhow::Result<()> {
    if should_skip() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <div id="app"></div>
                <script>
                    window.__clicks = 0;
                    function render(step) {
                        const rows = Array.from({length: (step % 5) + 1}, (_, i) =>
                            `<li data-step="${step}" data-index="${i}">row-${step}-${i}</li>`
                        ).join("");
                        document.getElementById("app").innerHTML = `
                            <section>
                                <h2>Checkout Flow</h2>
                                <ul>${rows}</ul>
                                <button id="target" onclick="window.__clicks = (window.__clicks || 0) + 1;">
                                    Confirm Purchase
                                </button>
                            </section>
                        `;
                    }
                    render(0);
                </script>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let root = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Interactive, &root)?;
    let state = SemanticState::new(sem, LoadProfile::Interactive);
    let (initial_target_id, stable_key) =
        find_node_info_by_dom_id(state.root(), "target").expect("target button not found");

    let iterations: i64 = 20;
    for step in 1..=iterations {
        page.evaluate_script(&format!("render({step});"))?;
        let stale_id = initial_target_id + 1_000_000 + step;
        page.act(Some(stale_id), Some(&stable_key), "click", None)?;
    }

    let clicks = page
        .evaluate_script("window.__clicks")?
        .value
        .and_then(|v| v.as_u64())
        .unwrap_or_default();
    assert_eq!(
        clicks, iterations as u64,
        "all fallback actions should succeed"
    );

    let fallback_recoveries = page
        .action_logs()?
        .into_iter()
        .filter(|entry| entry.code == "stable_key_fallback_recovered")
        .count();
    assert!(
        fallback_recoveries >= iterations as usize,
        "expected at least {iterations} fallback recovery logs, got {fallback_recoveries}"
    );

    Ok(())
}

fn find_node_info_by_dom_id(
    node: &core_runtime::sre::state::SemanticNode,
    dom_id: &str,
) -> Option<(i64, String)> {
    if node
        .attributes
        .as_ref()
        .and_then(|attrs| attrs.get("id"))
        .is_some_and(|id| id == dom_id)
    {
        return Some((node.backend_node_id, node.stable_key.clone()?));
    }

    for child in &node.children {
        if let Some(found) = find_node_info_by_dom_id(child, dom_id) {
            return Some(found);
        }
    }

    None
}
