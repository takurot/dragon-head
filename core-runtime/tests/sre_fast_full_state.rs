use core_runtime::{
    sre::{normalize_dom, LoadProfile, SemanticState, StateGenerationPhase},
    BrowserClient,
};

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
    if !core_runtime::chrome_available() {
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
    let expected_fast = state.generate_fast_state();
    let expected_full = state.generate_full_state();

    assert!(
        !layered.fast.interactive_elements.is_empty(),
        "Fast state interactive_elements must not be empty"
    );
    assert!(
        layered
            .fast
            .interactive_elements
            .iter()
            .any(|node| node.role == "input"),
        "Fast state must include input elements"
    );
    assert!(
        layered
            .fast
            .interactive_elements
            .iter()
            .any(|node| node.role == "button"),
        "Fast state must include button elements"
    );

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
    assert!(
        layered
            .fast
            .interactive_elements
            .iter()
            .all(|node| node.children.is_empty()),
        "Fast state nodes must not embed subtree children"
    );
    assert!(
        layered
            .fast
            .messages
            .iter()
            .all(|node| node.children.is_empty()),
        "Fast state message nodes must not embed subtree children"
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
    assert!(
        layered
            .full
            .forms
            .iter()
            .all(|node| node.children.is_empty()),
        "Full state form nodes must not embed subtree children"
    );
    assert!(
        layered
            .full
            .regions
            .iter()
            .all(|node| node.children.is_empty()),
        "Full state region nodes must not embed subtree children"
    );
    assert_eq!(
        layered.fast, expected_fast,
        "Layered fast state must match standalone fast state generation"
    );
    assert_eq!(
        layered.full, expected_full,
        "Layered full state must match standalone full state generation"
    );

    Ok(())
}

#[test]
fn test_fast_state_generated_before_full_state() -> anyhow::Result<()> {
    if !core_runtime::chrome_available() {
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
    let fast_only = state.generate_fast_state();
    let full_only = state.generate_full_state();

    assert_eq!(
        layered.generation_trace,
        vec![StateGenerationPhase::Fast, StateGenerationPhase::Full],
        "State generation must run fast phase before full phase"
    );
    assert_eq!(
        layered.fast, fast_only,
        "Layered output must use fast-phase generation result"
    );
    assert_eq!(
        layered.full, full_only,
        "Layered output must use full-phase generation result"
    );

    Ok(())
}
