use anyhow::Result;

use core_runtime::audit::AuditEvent;
use core_runtime::BrowserClient;
use mcp_server::{CoreRuntimeBackend, McpServer};
use serde_json::json;

/// ISSUE-187: `run_skill` must emit a top-level `TOOL_CALL` audit event for the
/// skill request itself, mirroring `act`/`verify_text`, even when the skill's
/// steps never reach an `act` call.
#[test]
fn run_skill_emits_top_level_tool_call_audit_event() -> Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    page.navigate("data:text/html,<html><body>hello</body></html>")?;

    let mut backend = CoreRuntimeBackend::new(page);
    backend.register_skill_json(&json!({
        "schema_version": 1,
        "name": "locate_only",
        "steps": [
            {
                "type": "locate",
                "query": "id:999999"
            }
        ]
    }))?;

    let mut server = McpServer::new(backend);
    server.backend_mut().page().clear_audit_events();

    // The locate step targets a nonexistent id, so the skill fails, but the
    // top-level run_skill request must still be recorded.
    server.call_tool("run_skill", json!({"skill_name": "locate_only"}))?;

    let events = server.backend_mut().page().audit_events();
    let found = events.iter().any(|event| {
        matches!(
            event,
            AuditEvent::ToolCall { tool_name, .. } if tool_name == "run_skill"
        )
    });
    assert!(
        found,
        "expected a TOOL_CALL(run_skill) audit event, got: {events:?}"
    );

    Ok(())
}
