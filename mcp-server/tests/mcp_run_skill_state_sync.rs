//! ISSUE-301: `run_skill`'s internal `act` steps mutate the live page directly through
//! `PageSkillRuntime`, bypassing the `state_cache` / speculative bookkeeping that
//! `CoreRuntimeBackend::act` maintains. These tests assert that `CoreRuntimeBackend` treats a
//! mutating skill run as an opaque invalidation boundary so a subsequent `get_state` cannot
//! return pre-skill state.
use core_runtime::BrowserClient;
use mcp_server::{CoreRuntimeBackend, McpServer};
use serde_json::{json, Value};

/// A page with a single button that swaps `document.body`'s content when clicked, producing a
/// distinct, stable `state_hash` before and after the click.
const SWAP_PAGE_HTML: &str = r#"
    <html>
        <head>
            <script>
                function showB() {
                    document.body.innerHTML = '<span id="after">After Click</span>';
                }
            </script>
        </head>
        <body>
            <button id="trigger" onclick="showB()">Before Click</button>
        </body>
    </html>
"#;

fn navigate_to_swap_page(page: &core_runtime::PageSession) -> anyhow::Result<()> {
    page.navigate(&format!(
        "data:text/html,{}",
        urlencoding::encode(SWAP_PAGE_HTML)
    ))?;
    Ok(())
}

/// Registers a skill that locates/verifies/clicks the first interactive element described by
/// `state`, returning the skill name.
fn register_click_skill(
    server: &mut McpServer<CoreRuntimeBackend>,
    state: &Value,
    name: &str,
) -> anyhow::Result<()> {
    let target_id = state["interactive_elements"][0]["id"]
        .as_i64()
        .expect("target id");
    let target = format!("id:{target_id}");

    server.backend_mut().register_skill_json(&json!({
        "schema_version": 1,
        "name": name,
        "steps": [
            { "type": "locate", "id": "loc", "query": target },
            { "type": "verify", "id": "ver", "target": target, "expected": "Before Click" },
            { "type": "act", "id": "click", "target": target, "action": "click" }
        ]
    }))?;
    Ok(())
}

#[test]
fn run_skill_mutation_invalidates_state_cache_for_next_get_state() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    navigate_to_swap_page(&page)?;

    let mut server = McpServer::new(CoreRuntimeBackend::new(page));

    // Populate `state_cache` with the pre-skill state (state A).
    let state_a = server.call_tool(
        "get_state",
        json!({ "format": "json", "force_refresh": true }),
    )?;
    assert_eq!(
        state_a["interactive_elements"][0]["role"],
        json!("button"),
        "unexpected initial state: {state_a}"
    );

    register_click_skill(&mut server, &state_a, "click_swap")?;
    let run_result = server.call_tool("run_skill", json!({"skill_name": "click_swap"}))?;
    assert_eq!(run_result["status"], json!("completed"), "{run_result}");

    // A plain `get_state` (no `force_refresh`) must not return the stale cached state A: the
    // skill's `act` step already changed the live DOM to state B.
    let state_b = server.call_tool("get_state", json!({ "format": "json" }))?;
    assert_ne!(
        state_a["metadata"]["state_hash"], state_b["metadata"]["state_hash"],
        "get_state returned the pre-skill state_cache after a mutating run_skill: {state_b}"
    );
    assert_eq!(
        state_b["metadata"]["speculative"],
        json!(false),
        "post-skill get_state must perform a real capture, not a speculative hit"
    );

    Ok(())
}

#[test]
fn run_skill_partial_failure_after_action_still_invalidates_state() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    navigate_to_swap_page(&page)?;

    let mut server = McpServer::new(CoreRuntimeBackend::new(page));

    let state_a = server.call_tool(
        "get_state",
        json!({ "format": "json", "force_refresh": true }),
    )?;
    let target_id = state_a["interactive_elements"][0]["id"]
        .as_i64()
        .expect("target id");
    let target = format!("id:{target_id}");

    // Skill clicks the button (a successful, page-mutating action) and then a later step
    // fails (locating an id that no longer exists once the DOM has been swapped).
    server.backend_mut().register_skill_json(&json!({
        "schema_version": 1,
        "name": "click_then_fail",
        "steps": [
            { "type": "locate", "id": "loc", "query": target },
            { "type": "verify", "id": "ver", "target": target, "expected": "Before Click" },
            { "type": "act", "id": "click", "target": target, "action": "click" },
            { "type": "locate", "id": "loc2", "query": "id:999999" }
        ]
    }))?;

    let run_result = server.call_tool("run_skill", json!({"skill_name": "click_then_fail"}))?;
    assert_eq!(run_result["status"], json!("failed"), "{run_result}");

    let state_b = server.call_tool("get_state", json!({ "format": "json" }))?;
    assert_ne!(
        state_a["metadata"]["state_hash"], state_b["metadata"]["state_hash"],
        "a successful act before a later skill failure must still invalidate state_cache: {state_b}"
    );

    Ok(())
}

#[test]
fn non_mutating_skill_does_not_invalidate_state_cache() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    navigate_to_swap_page(&page)?;

    let mut server = McpServer::new(CoreRuntimeBackend::new(page));

    let state_a = server.call_tool(
        "get_state",
        json!({ "format": "json", "force_refresh": true }),
    )?;
    let target_id = state_a["interactive_elements"][0]["id"]
        .as_i64()
        .expect("target id");
    let target = format!("id:{target_id}");

    server.backend_mut().register_skill_json(&json!({
        "schema_version": 1,
        "name": "verify_only",
        "steps": [
            { "type": "locate", "id": "loc", "query": target },
            { "type": "verify", "id": "ver", "target": target, "expected": "Before Click" }
        ]
    }))?;

    let run_result = server.call_tool("run_skill", json!({"skill_name": "verify_only"}))?;
    assert_eq!(run_result["status"], json!("completed"), "{run_result}");

    // A non-mutating skill (no `act` steps) must not discard the still-valid cached state.
    let state_b = server.call_tool("get_state", json!({ "format": "json" }))?;
    assert_eq!(
        state_a["metadata"]["state_hash"], state_b["metadata"]["state_hash"],
        "a non-mutating skill must not invalidate state_cache: {state_b}"
    );

    Ok(())
}
