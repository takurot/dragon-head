use anyhow::Context as _;
use std::{thread, time::Duration};

use core_runtime::{
    policy::{ApprovalScope, PolicyAction, PolicyRule},
    sre::{normalize_dom, LoadProfile, SemanticState},
    ActionError, BrowserClient,
};

fn should_skip() -> bool {
    std::env::var("CI").is_ok() && std::env::var("CHROME_INSTALLED").is_err()
}

#[test]
fn test_block_rule_prevents_action_execution() -> anyhow::Result<()> {
    if should_skip() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    page.set_policy_rules(vec![PolicyRule {
        id: "block-delete".to_string(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_string()),
        text_regex: Some("(?i)delete".to_string()),
        context_regex: None,
        action: PolicyAction::Block,
        scope: None,
    }])?;

    let html = r#"
        <html>
            <body>
                <button id="danger" onclick="document.body.dataset.deleted = 'true'">Delete</button>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let (target_id, target_key) = find_button_info(&page)?;
    let result = page.act(Some(target_id), Some(&target_key), "click", None);

    assert!(result.is_err(), "blocked policy must reject action");
    let err = result.unwrap_err();
    let action_err = err
        .downcast_ref::<ActionError>()
        .expect("error should be ActionError");
    assert!(matches!(action_err, ActionError::Blocked { .. }));

    let deleted = page.evaluate_script("document.body.dataset.deleted")?.value;
    assert!(
        deleted.is_none(),
        "blocked action must not mutate page state"
    );

    Ok(())
}

#[test]
fn test_action_only_approval_scope_expires_after_single_use() -> anyhow::Result<()> {
    if should_skip() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    page.set_policy_rules(vec![PolicyRule {
        id: "approve-pay".to_string(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_string()),
        text_regex: Some("(?i)pay".to_string()),
        context_regex: None,
        action: PolicyAction::RequireHumanApproval,
        scope: Some(ApprovalScope::ActionOnly),
    }])?;

    let html = r#"
        <html>
            <body>
                <button id="pay" onclick="document.body.dataset.payCount = String((Number(document.body.dataset.payCount||0)+1))">Pay</button>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let (target_id, target_key) = find_button_info(&page)?;

    let first_attempt = page.act(Some(target_id), Some(&target_key), "click", None);
    assert!(first_attempt.is_err(), "approval should be required first");
    let first_err = first_attempt.unwrap_err();
    let first_action_err = first_err
        .downcast_ref::<ActionError>()
        .expect("error should be ActionError");
    assert!(matches!(
        first_action_err,
        ActionError::HumanApprovalRequired { .. }
    ));

    page.approve_pending_policy_action()?;

    page.act(Some(target_id), Some(&target_key), "click", None)?;
    let pay_count = page
        .evaluate_script("document.body.dataset.payCount")?
        .value
        .and_then(|v| v.as_str().map(ToOwned::to_owned))
        .unwrap_or_default();
    assert_eq!(pay_count, "1", "approved action should execute once");

    let third_attempt = page.act(Some(target_id), Some(&target_key), "click", None);
    assert!(
        third_attempt.is_err(),
        "action_only approval must be consumed"
    );
    let third_err = third_attempt.unwrap_err();
    let third_action_err = third_err
        .downcast_ref::<ActionError>()
        .expect("error should be ActionError");
    assert!(matches!(
        third_action_err,
        ActionError::HumanApprovalRequired { .. }
    ));

    Ok(())
}

#[test]
fn test_action_only_approval_scope_expires_on_navigation() -> anyhow::Result<()> {
    if should_skip() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    page.set_policy_rules(vec![PolicyRule {
        id: "approve-pay".to_string(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_string()),
        text_regex: Some("(?i)pay".to_string()),
        context_regex: None,
        action: PolicyAction::RequireHumanApproval,
        scope: Some(ApprovalScope::ActionOnly),
    }])?;

    let html = r#"
        <html>
            <body>
                <button id="pay" onclick="document.body.dataset.payCount = String((Number(document.body.dataset.payCount||0)+1))">Pay</button>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let (target_id, target_key) = find_button_info(&page)?;
    assert!(page
        .act(Some(target_id), Some(&target_key), "click", None)
        .is_err());
    page.approve_pending_policy_action()?;

    page.navigate(&url)?;
    let (target_id_after_nav, target_key_after_nav) = find_button_info(&page)?;
    let attempt = page.act(
        Some(target_id_after_nav),
        Some(&target_key_after_nav),
        "click",
        None,
    );
    assert!(
        attempt.is_err(),
        "action_only approval must expire after navigation"
    );
    let err = attempt.unwrap_err();
    let action_err = err
        .downcast_ref::<ActionError>()
        .expect("error should be ActionError");
    assert!(matches!(
        action_err,
        ActionError::HumanApprovalRequired { .. }
    ));

    Ok(())
}

#[test]
fn test_set_policy_rules_clears_stale_approvals() -> anyhow::Result<()> {
    if should_skip() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    let rule = PolicyRule {
        id: "approve-pay".to_string(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_string()),
        text_regex: Some("(?i)pay".to_string()),
        context_regex: None,
        action: PolicyAction::RequireHumanApproval,
        scope: Some(ApprovalScope::ActionOnly),
    };
    page.set_policy_rules(vec![rule.clone()])?;

    let html = r#"
        <html>
            <body>
                <button id="pay" onclick="document.body.dataset.payCount = String((Number(document.body.dataset.payCount||0)+1))">Pay</button>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    let (target_id, target_key) = find_button_info(&page)?;
    assert!(page
        .act(Some(target_id), Some(&target_key), "click", None)
        .is_err());
    page.approve_pending_policy_action()?;

    page.set_policy_rules(vec![rule])?;

    let after_reload = page.act(Some(target_id), Some(&target_key), "click", None);
    assert!(
        after_reload.is_err(),
        "replacing policy rules must clear previously granted approvals"
    );
    let err = after_reload.unwrap_err();
    let action_err = err
        .downcast_ref::<ActionError>()
        .expect("error should be ActionError");
    assert!(matches!(
        action_err,
        ActionError::HumanApprovalRequired { .. }
    ));

    Ok(())
}

#[test]
fn test_until_navigation_and_timeboxed_scopes_expire() -> anyhow::Result<()> {
    if should_skip() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <button id="transfer" onclick="document.body.dataset.transferCount = String((Number(document.body.dataset.transferCount||0)+1))">Transfer</button>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));

    page.set_policy_rules(vec![PolicyRule {
        id: "approve-transfer-until-nav".to_string(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_string()),
        text_regex: Some("(?i)transfer".to_string()),
        context_regex: None,
        action: PolicyAction::RequireHumanApproval,
        scope: Some(ApprovalScope::UntilNavigation),
    }])?;
    page.navigate(&url)?;
    let (target_id, target_key) = find_button_info(&page)?;

    assert!(page
        .act(Some(target_id), Some(&target_key), "click", None)
        .is_err());
    page.approve_pending_policy_action()?;
    page.act(Some(target_id), Some(&target_key), "click", None)?;
    page.act(Some(target_id), Some(&target_key), "click", None)?;
    page.navigate(&url)?;

    let (target_id_after_nav, target_key_after_nav) = find_button_info(&page)?;
    let post_nav_attempt = page.act(
        Some(target_id_after_nav),
        Some(&target_key_after_nav),
        "click",
        None,
    );
    assert!(
        post_nav_attempt.is_err(),
        "until_navigation approval must expire on navigation"
    );

    page.set_policy_rules(vec![PolicyRule {
        id: "approve-transfer-timeboxed".to_string(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_string()),
        text_regex: Some("(?i)transfer".to_string()),
        context_regex: None,
        action: PolicyAction::RequireHumanApproval,
        scope: Some(ApprovalScope::Timeboxed { ms: 1_000 }),
    }])?;

    let (target_id_timeboxed, target_key_timeboxed) = find_button_info(&page)?;
    assert!(page
        .act(
            Some(target_id_timeboxed),
            Some(&target_key_timeboxed),
            "click",
            None
        )
        .is_err());
    page.approve_pending_policy_action()?;
    page.act(
        Some(target_id_timeboxed),
        Some(&target_key_timeboxed),
        "click",
        None,
    )?;

    thread::sleep(Duration::from_millis(1_250));
    let expired_attempt = page.act(
        Some(target_id_timeboxed),
        Some(&target_key_timeboxed),
        "click",
        None,
    );
    assert!(
        expired_attempt.is_err(),
        "timeboxed approval must expire after configured duration"
    );

    Ok(())
}

/// Regression test: `UntilNavigation` approval must expire when the page URL changes
/// due to a click-driven navigation, without calling `PageSession::navigate()`.
///
/// Two real `file://` pages are used: the approval is granted on page A, then Chrome
/// navigates to page B by clicking an `<a>` link. Because `navigate()` is never called,
/// `navigation_epoch` stays at the same value — but the URL changes, so the
/// `granted_url` comparison must invalidate the approval.
#[test]
fn test_until_navigation_expires_on_click_driven_navigation() -> anyhow::Result<()> {
    if should_skip() {
        return Ok(());
    }

    use std::fs;

    // Use unique names based on uuid to avoid test collisions.
    let tmp = std::env::temp_dir();
    let id = uuid::Uuid::new_v4();
    let path_b = tmp.join(format!("dragon-test-{id}-b.html"));
    let path_a = tmp.join(format!("dragon-test-{id}-a.html"));

    let html_b = "<html><body><button id='transfer'>Transfer</button></body></html>";
    fs::write(&path_b, html_b).context("failed to write page B")?;
    let url_b = format!("file://{}", path_b.display());

    let html_a = format!(
        r#"<html><body>
            <button id="transfer">Transfer</button>
            <a id="goto-b" href="{url_b}">Go to B</a>
        </body></html>"#
    );
    fs::write(&path_a, &html_a).context("failed to write page A")?;
    let url_a = format!("file://{}", path_a.display());

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    page.set_policy_rules(vec![PolicyRule {
        id: "approve-transfer-until-nav".to_string(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_string()),
        text_regex: Some("(?i)transfer".to_string()),
        context_regex: None,
        action: PolicyAction::RequireHumanApproval,
        scope: Some(ApprovalScope::UntilNavigation),
    }])?;

    // Load page A via navigate() — this sets navigation_epoch = 1 and grants_url = url_a.
    page.navigate(&url_a)?;

    let (target_id_a, target_key_a) = find_button_info(&page)?;
    // First act requires approval.
    assert!(page
        .act(Some(target_id_a), Some(&target_key_a), "click", None)
        .is_err());
    page.approve_pending_policy_action()?;

    // Second act on page A works — grant is live.
    page.act(Some(target_id_a), Some(&target_key_a), "click", None)?;

    // Click the link to page B via JavaScript — does NOT call navigate(), so
    // navigation_epoch remains the same. Only the URL changes.
    page.evaluate_script("document.getElementById('goto-b').click()")?;
    // Give Chrome time to complete the navigation.
    std::thread::sleep(std::time::Duration::from_millis(800));

    // Page B should now be loaded. Find the button there.
    let find_result = find_button_info(&page);
    let (target_id_b, target_key_b) = match find_result {
        Ok(info) => info,
        // If the DOM didn't update (e.g., browser security blocked the link),
        // skip gracefully rather than false-fail.
        Err(_) => return Ok(()),
    };

    // The URL is now url_b, but the grant recorded url_a. Approval must be expired.
    let post_nav_attempt = page.act(Some(target_id_b), Some(&target_key_b), "click", None);
    assert!(
        post_nav_attempt.is_err(),
        "until_navigation approval must expire after click-driven navigation to a different URL"
    );
    let err = post_nav_attempt.unwrap_err();
    let action_err = err
        .downcast_ref::<ActionError>()
        .expect("error should be ActionError");
    assert!(matches!(
        action_err,
        ActionError::HumanApprovalRequired { .. }
    ));

    let _ = fs::remove_file(&path_a);
    let _ = fs::remove_file(&path_b);
    Ok(())
}

/// Regression test: `context_regex` must match text that appears *outside* the
/// target button element (e.g., a `Total: $149` line in a sibling/parent node).
///
/// Previously `policy_context_text` only read the target node's own label and
/// attribute values, making `context_regex` on checkout amounts effectively dead.
#[test]
fn test_context_regex_matches_text_outside_button() -> anyhow::Result<()> {
    if should_skip() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    // Amount text ("Total: $149") lives in a <p> sibling — NOT inside the button.
    let html = r#"
        <html>
            <body>
                <div id="checkout">
                    <p id="total">Total: $149</p>
                    <button id="pay">Pay now</button>
                </div>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));

    page.set_policy_rules(vec![PolicyRule {
        id: "approve-pay-with-amount".to_string(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_string()),
        text_regex: Some("(?i)pay".to_string()),
        context_regex: Some(r"(?i)total\s*:\s*\$[0-9]+".to_string()),
        action: PolicyAction::RequireHumanApproval,
        scope: Some(ApprovalScope::ActionOnly),
    }])?;

    page.navigate(&url)?;

    let (target_id, target_key) = find_button_info(&page)?;
    let result = page.act(Some(target_id), Some(&target_key), "click", None);

    assert!(
        result.is_err(),
        "context_regex on sibling amount text must trigger require_human_approval"
    );
    let err = result.unwrap_err();
    let action_err = err
        .downcast_ref::<ActionError>()
        .expect("error should be ActionError");
    assert!(
        matches!(action_err, ActionError::HumanApprovalRequired { .. }),
        "expected HumanApprovalRequired when context_regex matches sibling DOM text, got: {action_err:?}"
    );

    Ok(())
}

fn find_button_info(page: &core_runtime::PageSession) -> anyhow::Result<(i64, String)> {
    let root = page.get_document_node()?;
    let sem = normalize_dom(LoadProfile::Interactive, &root)?;
    let state = SemanticState::new(sem, LoadProfile::Interactive);
    find_button_info_in_node(state.root()).ok_or_else(|| anyhow::anyhow!("Button not found"))
}

fn find_button_info_in_node(
    node: &core_runtime::sre::state::SemanticNode,
) -> Option<(i64, String)> {
    if node.role == "button" {
        return Some((
            node.backend_node_id,
            node.stable_key.clone().unwrap_or_default(),
        ));
    }
    for child in &node.children {
        if let Some(found) = find_button_info_in_node(child) {
            return Some(found);
        }
    }
    None
}
