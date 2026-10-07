//! ISSUE-257: `extract` explains *why* a rule produced nothing instead of returning a silent
//! null or an opaque "extraction script evaluation failed".

use core_runtime::BrowserClient;
use mcp_server::{CoreRuntimeBackend, McpServer};
use serde_json::{json, Value};

const FIXTURE: &str = r#"<html><body>
  <a class="x" href="/a">link</a>
  <div class="row"><span class="name">A</span><a href="/1">1</a></div>
  <div class="row"><span class="name">B</span><a href="/2">2</a></div>
  <div class="row"><a href="/3">3</a></div>
</body></html>"#;

fn server() -> anyhow::Result<McpServer<CoreRuntimeBackend>> {
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    page.navigate(&format!("data:text/html,{}", urlencoding::encode(FIXTURE)))?;
    Ok(McpServer::new(CoreRuntimeBackend::new_with_client(
        client, page,
    )))
}

fn extract(server: &mut McpServer<CoreRuntimeBackend>, args: Value) -> anyhow::Result<Value> {
    server.call_tool("extract", args)
}

macro_rules! browser_test {
    ($name:ident, |$server:ident| $body:block) => {
        #[test]
        fn $name() -> anyhow::Result<()> {
            if test_bench_support::should_skip_browser_tests() {
                return Ok(());
            }
            let mut $server = server()?;
            $body
            Ok(())
        }
    };
}

browser_test!(unmatched_selector_reports_selector_no_match, |s| {
    let out = extract(&mut s, json!({"inline": {"selector": ".missing"}}))?;
    assert_eq!(out["result"], Value::Null);
    assert_eq!(
        out["errors"]["$selector"],
        json!("SelectorNoMatch: No elements matched selector '.missing'")
    );
});

browser_test!(missing_attribute_reports_attribute_not_found, |s| {
    let out = extract(
        &mut s,
        json!({"inline": {"selector": "a.x", "attribute": "data-id"}}),
    )?;
    assert_eq!(out["result"], Value::Null);
    assert_eq!(
        out["errors"]["$selector"],
        json!("AttributeNotFound: Element matched selector 'a.x' has no attribute 'data-id'")
    );
});

browser_test!(partially_matching_field_reports_counts, |s| {
    let out = extract(
        &mut s,
        json!({"inline": {"items": {"selector": ".row", "fields": {
            "name": ".name", "href": "@href", "gone": ".gone"
        }}}}),
    )?;
    assert_eq!(out["result"][2]["name"], Value::Null);
    assert_eq!(
        out["errors"]["name"],
        json!("SelectorNoMatch: No elements matched selector '.name' within 1 of 3 items")
    );
    assert_eq!(
        out["errors"]["gone"],
        json!("SelectorNoMatch: No elements matched selector '.gone' within all 3 items")
    );
    // `.row` itself has no href attribute, so the item-level attribute is reported too.
    assert_eq!(
        out["errors"]["href"],
        json!("AttributeNotFound: No attribute 'href' within all 3 items")
    );
});

browser_test!(empty_item_selector_reports_rule_level_error, |s| {
    let out = extract(&mut s, json!({"rule_name": "headings"}))?;
    assert_eq!(out["result"], json!([]));
    assert_eq!(
        out["errors"]["$selector"],
        json!("SelectorNoMatch: No elements matched selector 'h1,h2,h3,h4,h5,h6'")
    );
});

browser_test!(
    invalid_selector_syntax_is_a_script_eval_error_with_the_cause,
    |s| {
        let err = extract(&mut s, json!({"inline": {"selector": "<<"}})).unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("ScriptEvalError"), "{message}");
        assert!(message.contains("not a valid selector"), "{message}");
    }
);

browser_test!(successful_extraction_has_no_errors_key_and_no_script, |s| {
    let out = extract(
        &mut s,
        json!({"inline": {"selector": "a.x", "attribute": "href"}}),
    )?;
    assert_eq!(out["result"], json!("/a"));
    assert!(out.get("errors").is_none(), "{out}");
    assert!(out.get("script").is_none(), "{out}");
});

browser_test!(debug_flag_returns_the_generated_script, |s| {
    let out = extract(
        &mut s,
        json!({"inline": {"selector": "a.x"}, "debug": true}),
    )?;
    let script = out["script"].as_str().expect("script string");
    assert!(
        script.contains("document.querySelector(\".x\")") || script.contains("a.x"),
        "{script}"
    );
    assert!(
        !script.contains("try {"),
        "debug shows the plain rule script: {script}"
    );
});
