//! ISSUE-259: the always-available `extract` rules work against a real page without any
//! inline DSL or registration.

use core_runtime::BrowserClient;
use mcp_server::{CoreRuntimeBackend, McpServer};
use serde_json::json;

const FIXTURE: &str = r#"<html>
  <head>
    <title>  Fixture Page  </title>
    <meta name="description" content="A page for built-in rules">
  </head>
  <body>
    <h1>Top</h1>
    <h2>Section <b>One</b></h2>
    <a href="/a">First</a>
    <a href="https://example.com/b"> Second </a>
    <a name="anchor-without-href">skipped</a>
  </body>
</html>"#;

fn server_for_fixture() -> anyhow::Result<McpServer<CoreRuntimeBackend>> {
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    page.navigate(&format!("data:text/html,{}", urlencoding::encode(FIXTURE)))?;
    Ok(McpServer::new(CoreRuntimeBackend::new_with_client(
        client, page,
    )))
}

#[test]
fn builtin_rules_extract_expected_values_from_a_real_page() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let mut server = server_for_fixture()?;
    let mut run = |rule: &str| server.call_tool("extract", json!({ "rule_name": rule }));

    assert_eq!(run("page_title")?["result"], json!("Fixture Page"));
    assert_eq!(
        run("meta_description")?["result"],
        json!("A page for built-in rules")
    );
    assert_eq!(
        run("all_links")?["result"],
        json!([
            {"text": "First", "href": "/a"},
            {"text": "Second", "href": "https://example.com/b"}
        ])
    );
    assert_eq!(
        run("headings")?["result"],
        json!([
            {"level": "H1", "text": "Top"},
            {"level": "H2", "text": "Section One"}
        ])
    );
    Ok(())
}

#[test]
fn unknown_rule_name_is_still_rejected() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let mut server = server_for_fixture()?;
    let err = server
        .call_tool("extract", json!({ "rule_name": "no_such_rule" }))
        .unwrap_err();
    assert!(err.to_string().contains("not found in registry"), "{err}");
    Ok(())
}
