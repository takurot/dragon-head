//! ISSUE-304: values written by an `extract` step were stored in `SkillExecutionContext::extracted`
//! but were dead data — unusable by later steps and unreturned from `run_skill`. These tests
//! prove, against a real Chrome session, that: an extracted value can be substituted into a
//! later step via `{{extracted.KEY}}`; `run_skill`'s JSON response returns `outputs`; the legacy
//! `{{param}}` behavior is unchanged; a missing/undefined `{{extracted.KEY}}` fails the step
//! instead of silently resolving; a page-derived extracted value cannot be used to choose an
//! action's verb; and extracted PII is redacted before it reaches `outputs`.
use core_runtime::BrowserClient;
use mcp_server::{CoreRuntimeBackend, McpServer};
use serde_json::json;

fn server_for_html(html: &str) -> anyhow::Result<McpServer<CoreRuntimeBackend>> {
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;
    // `new_with_client` (not `new`) retains `client` inside the backend — otherwise `client`
    // drops at the end of this function and takes the Chrome process down with it, since
    // `BrowserClient` owns the browser handle and `PageSession` alone doesn't keep it alive.
    Ok(McpServer::new(CoreRuntimeBackend::new_with_client(
        client, page,
    )))
}

// `<textarea>`, not `<input>`, so the target element has non-empty `innerText`/`textContent`
// ("READY") for the `verify` step every `act` step must be immediately preceded by (same target,
// per `skills_engine::validate_act_predecessors`) — an `<input>` always has empty text content,
// and the skill JSON schema requires `verify.expected` to be non-empty. `act`'s "type" action
// (`PageSession::act`) focuses (placing the cursor at the start) and simulates keystrokes
// without clearing existing content first, so the final value is the typed text *prepended* to
// "READY", not appended after it.
const EXTRACT_AND_TYPE_HTML: &str = r#"
    <html>
        <body>
            <div id="confirmation">ORD-9876</div>
            <textarea id="target-field">READY</textarea>
        </body>
    </html>
"#;

#[test]
fn extracted_value_is_usable_in_a_later_act_step() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let mut server = server_for_html(EXTRACT_AND_TYPE_HTML)?;

    let state = server.call_tool(
        "get_state",
        json!({"format": "json", "force_refresh": true}),
    )?;
    // The only interactive element on this fixture page is `#target-field`.
    let target_id = state["interactive_elements"][0]["id"]
        .as_i64()
        .expect("target id");
    let target = format!("id:{target_id}");

    server.backend_mut().register_skill_json(&json!({
        "schema_version": 1,
        "name": "extract_then_type",
        "steps": [
            {
                "type": "extract",
                "id": "extract_order",
                "key": "order_id",
                "selector": "#confirmation"
            },
            {
                "type": "locate",
                "id": "loc",
                "query": target
            },
            {
                "type": "verify",
                "id": "ver",
                "target": target,
                "expected": "READY"
            },
            {
                "type": "act",
                "id": "type_it",
                "action": "type",
                "target": target,
                "value": "{{extracted.order_id}}"
            }
        ]
    }))?;

    let run_result = server.call_tool("run_skill", json!({"skill_name": "extract_then_type"}))?;
    assert_eq!(run_result["status"], json!("completed"), "{run_result}");
    assert_eq!(
        run_result["outputs"]["order_id"],
        json!("ORD-9876"),
        "{run_result}"
    );

    let field_value = server
        .backend_mut()
        .page()
        .evaluate_script("document.getElementById('target-field').value")?
        .value
        .and_then(|v| v.as_str().map(ToOwned::to_owned));
    assert_eq!(
        field_value,
        Some("ORD-9876READY".to_string()),
        "the extracted value must actually have been typed into the field"
    );

    Ok(())
}

#[test]
fn legacy_bare_param_template_remains_unchanged() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let html = r#"<html><body><textarea id="email-field">READY</textarea></body></html>"#;
    let mut server = server_for_html(html)?;

    let state = server.call_tool(
        "get_state",
        json!({"format": "json", "force_refresh": true}),
    )?;
    let target_id = state["interactive_elements"][0]["id"]
        .as_i64()
        .expect("target id");
    let target = format!("id:{target_id}");

    server.backend_mut().register_skill_json(&json!({
        "schema_version": 1,
        "name": "legacy_param_skill",
        "steps": [
            { "type": "locate", "id": "loc", "query": target },
            { "type": "verify", "id": "ver", "target": target, "expected": "READY" },
            {
                "type": "act",
                "id": "type_it",
                "action": "type",
                "target": target,
                "value": "{{email}}"
            }
        ]
    }))?;

    let run_result = server.call_tool(
        "run_skill",
        json!({"skill_name": "legacy_param_skill", "params": {"email": "legacy@example.com"}}),
    )?;
    assert_eq!(run_result["status"], json!("completed"), "{run_result}");

    let field_value = server
        .backend_mut()
        .page()
        .evaluate_script("document.getElementById('email-field').value")?
        .value
        .and_then(|v| v.as_str().map(ToOwned::to_owned));
    assert_eq!(field_value, Some("legacy@example.comREADY".to_string()));

    Ok(())
}

#[test]
fn undefined_extracted_reference_fails_the_step_with_a_clear_error() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let html = r#"<html><body><textarea id="field">READY</textarea></body></html>"#;
    let mut server = server_for_html(html)?;

    let state = server.call_tool(
        "get_state",
        json!({"format": "json", "force_refresh": true}),
    )?;
    let target_id = state["interactive_elements"][0]["id"]
        .as_i64()
        .expect("target id");
    let target = format!("id:{target_id}");

    // No `extract` step ever produces `order_id` — reference it directly.
    server.backend_mut().register_skill_json(&json!({
        "schema_version": 1,
        "name": "missing_extracted",
        "steps": [
            { "type": "locate", "id": "loc", "query": target },
            { "type": "verify", "id": "ver", "target": target, "expected": "READY" },
            {
                "type": "act",
                "id": "type_it",
                "action": "type",
                "target": target,
                "value": "{{extracted.order_id}}"
            }
        ]
    }))?;

    let run_result = server.call_tool("run_skill", json!({"skill_name": "missing_extracted"}))?;
    assert_eq!(run_result["status"], json!("failed"), "{run_result}");
    let message = run_result["message"].as_str().unwrap_or_default();
    assert!(message.contains("extracted.order_id"), "{run_result}");

    Ok(())
}

#[test]
fn extracted_value_cannot_choose_an_action_verb() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let html = r#"
        <html>
            <body>
                <div id="verb">click</div>
                <button id="btn">Go</button>
            </body>
        </html>
    "#;
    let mut server = server_for_html(html)?;

    let state = server.call_tool(
        "get_state",
        json!({"format": "json", "force_refresh": true}),
    )?;
    let button = state["interactive_elements"]
        .as_array()
        .expect("interactive_elements")
        .iter()
        .find(|el| el["role"] == json!("button"))
        .expect("button not found");
    let target = format!("id:{}", button["id"].as_i64().expect("target id"));

    server.backend_mut().register_skill_json(&json!({
        "schema_version": 1,
        "name": "extracted_verb",
        "steps": [
            {
                "type": "extract",
                "id": "extract_verb",
                "key": "verb",
                "selector": "#verb"
            },
            {
                "type": "locate",
                "id": "loc",
                "query": target
            },
            {
                "type": "verify",
                "id": "ver",
                "target": target,
                "expected": "Go"
            },
            {
                "type": "act",
                "id": "act_it",
                "action": "{{extracted.verb}}",
                "target": target
            }
        ]
    }))?;

    let run_result = server.call_tool("run_skill", json!({"skill_name": "extracted_verb"}))?;
    assert_eq!(run_result["status"], json!("failed"), "{run_result}");
    let message = run_result["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("extracted.verb"),
        "extracted.* must be rejected in an act.action (control) slot: {run_result}"
    );

    Ok(())
}

#[test]
fn extracted_pii_is_redacted_in_run_skill_outputs() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let html = r#"<html><body><div id="contact">reach me at agent@example.com</div></body></html>"#;
    let mut server = server_for_html(html)?;

    server.backend_mut().register_skill_json(&json!({
        "schema_version": 1,
        "name": "extract_pii",
        "steps": [
            {
                "type": "extract",
                "id": "extract_contact",
                "key": "contact",
                "selector": "#contact"
            }
        ]
    }))?;

    let run_result = server.call_tool("run_skill", json!({"skill_name": "extract_pii"}))?;
    assert_eq!(run_result["status"], json!("completed"), "{run_result}");
    let contact = run_result["outputs"]["contact"]
        .as_str()
        .expect("contact output");
    assert!(
        !contact.contains("agent@example.com"),
        "raw email must not reach run_skill outputs unredacted: {run_result}"
    );
    // `core_runtime::privacy::PiiRedactor::redact_text` replaces emails with "***"
    // (see `core-runtime/src/privacy.rs`), not a labeled placeholder.
    assert!(contact.contains("***"), "{run_result}");

    Ok(())
}

#[test]
fn selector_miss_stores_null_and_fails_a_later_reference() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let html = r#"<html><body><textarea id="field">READY</textarea></body></html>"#;
    let mut server = server_for_html(html)?;

    let state = server.call_tool(
        "get_state",
        json!({"format": "json", "force_refresh": true}),
    )?;
    let target_id = state["interactive_elements"][0]["id"]
        .as_i64()
        .expect("target id");
    let target = format!("id:{target_id}");

    server.backend_mut().register_skill_json(&json!({
        "schema_version": 1,
        "name": "extract_miss_then_use",
        "steps": [
            {
                "type": "extract",
                "id": "extract_missing",
                "key": "does_not_exist",
                "selector": "#no-such-element"
            },
            {
                "type": "locate",
                "id": "loc",
                "query": target
            },
            {
                "type": "verify",
                "id": "ver",
                "target": target,
                "expected": "READY"
            },
            {
                "type": "act",
                "id": "type_it",
                "action": "type",
                "target": target,
                "value": "{{extracted.does_not_exist}}"
            }
        ]
    }))?;

    let run_result =
        server.call_tool("run_skill", json!({"skill_name": "extract_miss_then_use"}))?;
    assert_eq!(run_result["status"], json!("failed"), "{run_result}");
    assert_eq!(
        run_result["outputs"]["does_not_exist"],
        json!(null),
        "a selector miss must store null, not an empty string: {run_result}"
    );

    Ok(())
}
