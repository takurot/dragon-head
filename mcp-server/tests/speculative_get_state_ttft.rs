//! End-to-end MCP test for the Speculative State Generation pipeline wiring
//! (Spec §3.5 / ISSUE-147).
//!
//! Walks a two-page cycle (A -> B -> A -> B -> A) via `act` + `get_state`
//! calls through the MCP layer. After enough repetitions, the speculative
//! engine learns the action sequence and the final `get_state` call is served
//! from a verified pre-generated snapshot (`metadata.speculative == true`),
//! bypassing the CDP DOM capture entirely. The bypass is verified directly
//! via `metadata.speculative` and the `get_usage_report` counters rather than
//! by comparing elapsed wall-clock time, which is prone to scheduler jitter
//! on loaded or virtualized CI hosts (Spec §3.5 / ISSUE-147 round-9 review).
use core_runtime::BrowserClient;
use mcp_server::{CoreRuntimeBackend, McpServer};
use serde_json::json;

#[test]
fn repeat_traversal_serves_speculative_hit_without_real_capture() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    // Page A has a single "Show Details" button that swaps the body for page
    // B; page B has a single "Back to List" button that swaps back to page A.
    // Each swap fully replaces `document.body`'s content, so the two pages
    // produce distinct, stable `state_hash`es across repeat visits.
    let html = r#"
        <html>
            <head>
                <script>
                    function showA() {
                        document.body.innerHTML =
                            '<button id="show-details" onclick="showB()">Show Details</button>';
                    }
                    function showB() {
                        document.body.innerHTML =
                            '<button id="back-to-list" onclick="showA()">Back to List</button>';
                    }
                </script>
            </head>
            <body>
                <button id="show-details" onclick="showB()">Show Details</button>
            </body>
        </html>
    "#;
    page.navigate(&format!("data:text/html,{}", urlencoding::encode(html)))?;

    let mut server = McpServer::new(CoreRuntimeBackend::new(page));

    let click_current_button = |server: &mut McpServer<CoreRuntimeBackend>,
                                state: &serde_json::Value|
     -> anyhow::Result<()> {
        let target_id = state["interactive_elements"][0]["id"]
            .as_i64()
            .expect("target id");
        let stable_key = state["interactive_elements"][0]["stable_key"]
            .as_str()
            .expect("stable key")
            .to_string();
        let act_result = server.call_tool(
            "act",
            json!({
                "target_id": target_id,
                "target_stable_key": stable_key,
                "action": "click"
            }),
        )?;
        assert_eq!(act_result["status"], json!("ok"));
        Ok(())
    };

    // Call 1: initial capture (page A), force_refresh to bypass the cache.
    let state_1 = server.call_tool(
        "get_state",
        json!({ "format": "json", "force_refresh": true }),
    )?;
    assert_eq!(state_1["metadata"]["speculative"], json!(false));
    click_current_button(&mut server, &state_1)?; // -> page B

    // Call 3: full capture of page B (a speculative miss).
    let state_3 = server.call_tool("get_state", json!({ "format": "json" }))?;
    assert_eq!(state_3["metadata"]["speculative"], json!(false));
    click_current_button(&mut server, &state_3)?; // -> page A

    // Call 5: full capture of page A (second speculative miss).
    let state_5 = server.call_tool("get_state", json!({ "format": "json" }))?;
    assert_eq!(state_5["metadata"]["speculative"], json!(false));
    click_current_button(&mut server, &state_5)?; // -> page B

    // Call 7: full capture of page B (third speculative miss — by now the
    // engine has observed A->B and B->A often enough to predict the cycle).
    let state_7 = server.call_tool("get_state", json!({ "format": "json" }))?;
    assert_eq!(state_7["metadata"]["speculative"], json!(false));
    click_current_button(&mut server, &state_7)?; // -> page A

    // Call 9: the engine should now predict "back-to-list, then show-details"
    // and serve the cached page-A snapshot directly — a speculative hit.
    let state_9 = server.call_tool("get_state", json!({ "format": "json" }))?;

    assert_eq!(
        state_9["metadata"]["speculative"],
        json!(true),
        "expected the final get_state call to be served from a verified speculative \
         pre-generation; full response: {state_9}"
    );
    assert_eq!(
        state_9["metadata"]["state_hash"], state_5["metadata"]["state_hash"],
        "the speculative snapshot must match the previously observed page-A state"
    );
    assert_eq!(
        state_9["interactive_elements"][0]["alias"],
        state_5["interactive_elements"][0]["alias"]
    );

    let usage_report = server.call_tool("get_usage_report", json!({}))?;
    assert_eq!(usage_report["state_generations"]["speculative"], json!(1));
    assert_eq!(usage_report["speculative_misses"], json!(3));

    Ok(())
}
