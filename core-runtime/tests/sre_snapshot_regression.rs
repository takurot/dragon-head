use anyhow::{Context, Result};
use core_runtime::{
    sre::{normalize_dom, LoadProfile, SemanticNode, SemanticState},
    BrowserClient,
};
use serde_json::Value;
use std::{fs, path::PathBuf};

const SNAPSHOT_REL_PATH: &str = "tests/fixtures/sre/minimal_regression_snapshot.json";
const UPDATE_ENV: &str = "UPDATE_SRE_SNAPSHOTS";

fn should_skip() -> bool {
    std::env::var("CI").is_ok() && std::env::var("CHROME_INSTALLED").is_err()
}

fn snapshot_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(SNAPSHOT_REL_PATH)
}

fn canonicalize_node(mut node: SemanticNode) -> SemanticNode {
    node.backend_node_id = 0;
    node.children = node.children.into_iter().map(canonicalize_node).collect();
    node
}

fn build_snapshot() -> Result<Value> {
    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html_content = r#"
        <html>
            <body>
                <main class="app shell-v12345">
                    <h1>Checkout</h1>
                    <form id="payment-form">
                        <label for="email">Email</label>
                        <input id="email" type="email" />
                        <button class="btn sc-abcd1234">Pay now</button>
                    </form>
                </main>
            </body>
        </html>
    "#;

    let url = format!("data:text/html,{}", urlencoding::encode(html_content));
    page.navigate(&url)?;

    let node = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Minimal, &node)?;
    let state = SemanticState::new(sem, LoadProfile::Minimal);
    let root = canonicalize_node(state.root().clone());

    Ok(serde_json::json!({
        "state_hash": state.state_hash(),
        "root": root
    }))
}

#[test]
fn test_sre_minimal_snapshot_regression() -> Result<()> {
    if should_skip() {
        return Ok(());
    }

    let path = snapshot_path();
    let actual = build_snapshot()?;

    if std::env::var(UPDATE_ENV).as_deref() == Ok("1") {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create snapshot directory: {}", parent.display())
            })?;
        }
        let serialized =
            serde_json::to_string_pretty(&actual).context("Failed to serialize snapshot")?;
        fs::write(&path, serialized)
            .with_context(|| format!("Failed to write snapshot: {}", path.display()))?;
        return Ok(());
    }

    let expected_text = fs::read_to_string(&path).with_context(|| {
        format!(
            "Snapshot file missing. Generate it with `{UPDATE_ENV}=1 cargo test -p core-runtime --test sre_snapshot_regression` at {}",
            path.display()
        )
    })?;
    let expected: Value = serde_json::from_str(&expected_text)
        .with_context(|| format!("Failed to parse snapshot JSON: {}", path.display()))?;

    assert_eq!(
        actual, expected,
        "SRE snapshot changed. If intentional, update with `{UPDATE_ENV}=1 cargo test -p core-runtime --test sre_snapshot_regression`."
    );
    Ok(())
}
