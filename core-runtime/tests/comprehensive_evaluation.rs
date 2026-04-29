use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Context;
use core_runtime::should_skip_browser_tests;
use core_runtime::{
    audit::AuditEvent,
    sre::{normalize_dom, LoadProfile, SemanticState},
    ActionError, BrowserClient, KmsAdapter, LocalSessionVault, PageSession, PolicyAction,
    PolicyRule, SoftwareKms,
};
use serde_json::{json, Value};
use test_bench_support::{EvaluationBench, EvaluationMode};

#[test]
fn test_core_runtime_comprehensive_evaluation_suite() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let mut bench = EvaluationBench::new(
        "core-runtime",
        "comprehensive_evaluation",
        EvaluationMode::from_env(),
    );

    bench.run_scenario(
        "semantic_state_capture",
        "sre",
        scenario_semantic_state_capture,
    );
    bench.run_scenario(
        "stable_key_recovery",
        "actions",
        scenario_stable_key_recovery,
    );
    bench.run_scenario("semantic_wait", "waits", scenario_semantic_wait);
    bench.run_scenario("policy_hitl", "policy", scenario_policy_hitl);
    bench.run_scenario("audit_redaction", "audit", scenario_audit_redaction);
    bench.run_scenario(
        "session_vault_roundtrip",
        "session_vault",
        scenario_session_vault_roundtrip,
    );

    bench.write_if_configured()?;
    bench.assert_required_scenarios(&[
        "semantic_state_capture",
        "stable_key_recovery",
        "semantic_wait",
        "policy_hitl",
        "audit_redaction",
        "session_vault_roundtrip",
    ])?;
    bench.assert_all_passed()?;

    Ok(())
}

fn scenario_semantic_state_capture() -> anyhow::Result<Value> {
    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <h1>Checkout</h1>
                <button id="purchase">Purchase</button>
                <div role="status">Ready</div>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let state = page.capture_semantic_state(LoadProfile::Interactive)?;
    let fast_state = state.generate_fast_state();
    let purchase = fast_state
        .interactive_elements
        .iter()
        .find(|node| {
            node.role == "button"
                && node
                    .attributes
                    .as_ref()
                    .and_then(|attrs| attrs.get("id"))
                    .is_some_and(|id| id == "purchase")
        })
        .context("purchase button missing from fast state")?;

    let indexed = page.refresh_stable_key_index(LoadProfile::Interactive)?;
    let backend_id = purchase.backend_node_id;
    let stable_key = purchase
        .stable_key
        .as_ref()
        .context("purchase button stable_key missing")?;

    assert!(
        indexed > 0,
        "stable key index should contain at least one interactive element"
    );
    assert_eq!(
        page.lookup_backend_node_id_by_stable_key(stable_key),
        Some(backend_id)
    );

    Ok(json!({
        "interactive_elements": fast_state.interactive_elements.len(),
        "indexed_elements": indexed,
        "purchase_stable_key": stable_key,
    }))
}

fn scenario_stable_key_recovery() -> anyhow::Result<Value> {
    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <div id="container">
                    <button id="btn" onclick="document.body.dataset.clicked='true'">Target</button>
                </div>
                <script>
                    function replaceButton() {
                        const container = document.getElementById('container');
                        container.innerHTML = '<button id="btn" onclick="document.body.dataset.clicked = \'true\'">Target</button>';
                    }
                </script>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let root = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Minimal, &root)?;
    let state = SemanticState::new(sem, LoadProfile::Minimal);
    let (_, stable_key) = find_button_info(state.root()).context("button not found")?;

    page.evaluate_script("replaceButton()")?;
    page.act(Some(999_999), Some(&stable_key), "click", None)?;

    let clicked = page.evaluate_script("document.body.dataset.clicked")?.value;
    assert_eq!(
        clicked
            .and_then(|value| value.as_str().map(ToOwned::to_owned))
            .as_deref(),
        Some("true")
    );

    let recovered = page.action_logs()?.into_iter().any(|entry| {
        entry.code == "stable_key_fallback_recovered"
            && entry.stable_key.as_deref() == Some(stable_key.as_str())
    });
    assert!(
        recovered,
        "stable_key fallback should emit recovery warning"
    );

    Ok(json!({
        "stable_key": stable_key,
        "recovered": recovered,
    }))
}

fn scenario_semantic_wait() -> anyhow::Result<Value> {
    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <button id="btn_login" disabled>Login</button>
                <script>
                    setTimeout(() => {
                        document.getElementById("btn_login").removeAttribute("disabled");
                    }, 250);
                </script>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let root = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Interactive, &root)?;
    let state = SemanticState::new(sem, LoadProfile::Interactive);
    let stable_key = find_button_info(state.root())
        .map(|(_, k)| k)
        .context("login button stable_key missing")?;

    page.wait_for_semantic(
        core_runtime::SemanticTarget::StableKey(stable_key.clone()),
        core_runtime::SemanticWaitState::Enabled,
        Duration::from_secs(2),
    )?;

    let is_disabled = page
        .evaluate_script(r#"document.getElementById("btn_login").hasAttribute("disabled")"#)?
        .value
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    assert!(!is_disabled, "button should become enabled");

    Ok(json!({
        "stable_key": stable_key,
        "enabled": true,
    }))
}

fn scenario_policy_hitl() -> anyhow::Result<Value> {
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    page.set_policy_rules(vec![PolicyRule {
        id: "purchase-approval".to_string(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_string()),
        text_regex: Some("(?i)purchase".to_string()),
        context_regex: None,
        action: PolicyAction::RequireHumanApproval,
        scope: Some(core_runtime::ApprovalScope::ActionOnly),
    }])?;

    let html = r#"
        <html>
            <body>
                <button id="purchase" onclick="document.body.dataset.approved='yes'">Purchase</button>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let (target_id, stable_key) = find_button_info_from_page(&page)?;
    let first_attempt = page.act(Some(target_id), Some(&stable_key), "click", None);
    let first_error = first_attempt.expect_err("approval should be required");
    let action_error = first_error
        .downcast_ref::<ActionError>()
        .context("expected ActionError")?;
    assert!(matches!(
        action_error,
        ActionError::HumanApprovalRequired { .. }
    ));

    page.approve_pending_policy_action()?;
    page.act(Some(target_id), Some(&stable_key), "click", None)?;

    let approved = page
        .evaluate_script("document.body.dataset.approved")?
        .value
        .and_then(|value| value.as_str().map(ToOwned::to_owned));
    assert_eq!(approved.as_deref(), Some("yes"));

    Ok(json!({
        "target_id": target_id,
        "stable_key": stable_key,
        "approved": true,
    }))
}

fn scenario_audit_redaction() -> anyhow::Result<Value> {
    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <input id="email" type="email" value="seed@example.com" />
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let root = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Interactive, &root)?;
    let state = SemanticState::new(sem, LoadProfile::Interactive);
    let (target_id, stable_key) =
        find_node_info_by_dom_id(state.root(), "email").context("email input not found")?;

    page.clear_audit_events();
    page.act(
        Some(target_id),
        Some(&stable_key),
        "type",
        Some("alice@example.com 4000 1234 5678 9012"),
    )?;

    let tool_args = page
        .audit_events()
        .into_iter()
        .find_map(|event| match event {
            AuditEvent::ToolCall {
                tool_name, args, ..
            } if tool_name == "act" => Some(args),
            _ => None,
        })
        .context("TOOL_CALL(act) missing")?;
    let tool_args_text = serde_json::to_string(&tool_args)?;

    assert!(!tool_args_text.contains("alice@example.com"));
    assert!(!tool_args_text.contains("4000 1234 5678 9012"));
    assert_eq!(tool_args["value"], json!("***"));

    Ok(json!({
        "masked_value": tool_args["value"],
    }))
}

fn scenario_session_vault_roundtrip() -> anyhow::Result<Value> {
    let kms_log = Arc::new(Mutex::new(KmsCallLog::default()));
    let vault = Arc::new(LocalSessionVault::new(Box::new(RecordingKms::new(
        [1u8; 32],
        "key-1".to_string(),
        Arc::clone(&kms_log),
    ))));
    let client = BrowserClient::new_with_vault(vault)?;
    let page = client.new_page()?;
    let runtime = tokio::runtime::Runtime::new()?;

    page.navigate("https://example.com")?;
    set_cookie(&page, "session_com", "alpha")?;
    runtime.block_on(page.save_to_vault("session-example-com"))?;

    clear_cookie(&page, "session_com")?;
    assert!(cookie_value(&page, "session_com")?.is_none());

    runtime.block_on(page.load_from_vault("session-example-com"))?;
    assert_eq!(
        cookie_value(&page, "session_com")?.as_deref(),
        Some("alpha")
    );
    assert_eq!(last_encrypt_key_id(&kms_log).as_deref(), Some("key-1"));

    Ok(json!({
        "session_id": "session-example-com",
        "encrypt_key_id": last_encrypt_key_id(&kms_log),
    }))
}

fn find_button_info(node: &core_runtime::sre::state::SemanticNode) -> Option<(i64, String)> {
    if node.role == "button" {
        return Some((node.backend_node_id, node.stable_key.clone()?));
    }
    for child in &node.children {
        if let Some(found) = find_button_info(child) {
            return Some(found);
        }
    }
    None
}

fn find_button_info_from_page(page: &PageSession) -> anyhow::Result<(i64, String)> {
    let root = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Interactive, &root)?;
    let state = SemanticState::new(sem, LoadProfile::Interactive);
    find_button_info(state.root()).context("button not found")
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

fn set_cookie(page: &PageSession, name: &str, value: &str) -> anyhow::Result<()> {
    let script = format!(
        r#"document.cookie = "{}={}; path=/; max-age=3600";"#,
        name, value
    );
    page.evaluate_script(&script)?;
    Ok(())
}

fn clear_cookie(page: &PageSession, name: &str) -> anyhow::Result<()> {
    let script = format!(
        r#"document.cookie = "{}=; path=/; expires=Thu, 01 Jan 1970 00:00:00 GMT";"#,
        name
    );
    page.evaluate_script(&script)?;
    Ok(())
}

fn cookie_value(page: &PageSession, name: &str) -> anyhow::Result<Option<String>> {
    let raw = page
        .evaluate_script("document.cookie")?
        .value
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .context("document.cookie should return string")?;

    Ok(raw.split(';').find_map(|entry| {
        let mut parts = entry.trim().splitn(2, '=');
        let key = parts.next()?;
        let value = parts.next().unwrap_or_default();
        (key == name).then(|| value.to_string())
    }))
}

#[derive(Debug, Default)]
struct KmsCallLog {
    encrypt_key_ids: Vec<String>,
}

struct RecordingKms {
    inner: SoftwareKms,
    log: Arc<Mutex<KmsCallLog>>,
}

impl RecordingKms {
    fn new(key: [u8; 32], key_id: String, log: Arc<Mutex<KmsCallLog>>) -> Self {
        Self {
            inner: SoftwareKms::new(key, key_id),
            log,
        }
    }
}

#[async_trait::async_trait]
impl KmsAdapter for RecordingKms {
    async fn encrypt(&self, plaintext: &[u8]) -> anyhow::Result<(Vec<u8>, String)> {
        let (ciphertext, key_id) = self.inner.encrypt(plaintext).await?;
        self.log
            .lock()
            .expect("kms log lock should not be poisoned")
            .encrypt_key_ids
            .push(key_id.clone());
        Ok((ciphertext, key_id))
    }

    async fn decrypt(&self, ciphertext: &[u8], key_id: &str) -> anyhow::Result<Vec<u8>> {
        self.inner.decrypt(ciphertext, key_id).await
    }

    fn current_key_id(&self) -> String {
        self.inner.current_key_id()
    }

    fn add_key(&mut self, key: [u8; 32], key_id: String, make_current: bool) {
        self.inner.add_key(key, key_id, make_current);
    }
}

fn last_encrypt_key_id(log: &Arc<Mutex<KmsCallLog>>) -> Option<String> {
    log.lock()
        .expect("kms log lock should not be poisoned")
        .encrypt_key_ids
        .last()
        .cloned()
}
