use core_runtime::{
    sre::{normalize_dom, LoadProfile, SemanticState, StateGenerationPhase},
    BrowserClient,
};

fn should_skip() -> bool {
    std::env::var("CI").is_ok() && std::env::var("CHROME_INSTALLED").is_err()
}

fn build_state(html: &str) -> anyhow::Result<SemanticState> {
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let root_node = page.get_document_node()?;
    let normalized = normalize_dom(LoadProfile::Minimal, &root_node)?;
    Ok(SemanticState::new(normalized, LoadProfile::Minimal))
}

#[test]
fn test_fast_full_state_content_diff() -> anyhow::Result<()> {
    if should_skip() {
        return Ok(());
    }

    let html = r#"
        <html>
            <body>
                <div id="status">Order ready</div>
                <section role="region" aria-label="checkout-area">
                    <form id="checkout-form">
                        <input type="email" id="email" />
                        <button type="submit">Purchase</button>
                    </form>
                </section>
            </body>
        </html>
    "#;

    let state = build_state(html)?;
    let layered = state.generate_layered_state();

    assert!(
        layered
            .fast
            .interactive_elements
            .iter()
            .all(|node| node.role != "form"),
        "Fast state must contain interactive elements only"
    );
    assert!(
        layered.fast.messages.iter().all(|node| node.role == "text"),
        "Fast state messages must contain only text nodes"
    );

    let has_order_message = layered.fast.messages.iter().any(|node| {
        node.label
            .as_deref()
            .map(|text| text.contains("Order ready"))
            .unwrap_or(false)
    });
    assert!(
        has_order_message,
        "Fast state must capture visible messages"
    );

    let has_form = layered.full.forms.iter().any(|node| node.role == "form");
    assert!(has_form, "Full state must include form nodes");

    let has_region = layered.full.regions.iter().any(|node| {
        node.attributes
            .as_ref()
            .and_then(|attrs| attrs.get("role"))
            .map(|role| role == "region")
            .unwrap_or(false)
    });
    assert!(has_region, "Full state must include region nodes");

    Ok(())
}

#[test]
fn test_fast_state_generated_before_full_state() -> anyhow::Result<()> {
    if should_skip() {
        return Ok(());
    }

    let html = r#"
        <html>
            <body>
                <section role="region">
                    <form>
                        <input type="text" />
                    </form>
                </section>
            </body>
        </html>
    "#;

    let state = build_state(html)?;
    let layered = state.generate_layered_state();

    assert_eq!(
        layered.generation_trace,
        vec![StateGenerationPhase::Fast, StateGenerationPhase::Full],
        "State generation must run fast phase before full phase"
    );

    Ok(())
}
