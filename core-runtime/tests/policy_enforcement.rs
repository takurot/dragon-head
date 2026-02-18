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
