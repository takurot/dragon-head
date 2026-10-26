//! ISSUE-396: element approvals belong to the document reviewed by the human.

use anyhow::Context;
use core_runtime::{
    audit::AuditEvent,
    policy::{ApprovalScope, PolicyAction, PolicyRule},
    sre::{LoadProfile, SemanticNode},
    ActionError, BrowserClient, PageSession,
};
#[path = "support/approval_fixture.rs"]
mod approval_fixture;
use approval_fixture::Fixture;
use std::{
    thread,
    time::{Duration, Instant},
};

fn rule(scope: ApprovalScope) -> PolicyRule {
    PolicyRule {
        id: "approve-delete".to_string(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_string()),
        text_regex: None,
        context_regex: None,
        action: PolicyAction::RequireHumanApproval,
        scope: Some(scope),
        outcome_projector: None,
    }
}

fn button(page: &PageSession) -> anyhow::Result<(i64, String)> {
    fn find(node: &SemanticNode) -> Option<(i64, String)> {
        if node.role == "button" {
            return node
                .stable_key
                .clone()
                .map(|key| (node.backend_node_id, key));
        }
        node.children.iter().find_map(find)
    }
    let state = page.capture_semantic_state(LoadProfile::Interactive)?;
    find(state.root()).ok_or_else(|| anyhow::anyhow!("fixture button is missing"))
}

fn require_approval(page: &PageSession, phase: &str) -> anyhow::Result<String> {
    let (id, key) = button(page)?;
    let error = page
        .act(Some(id), Some(&key), "click", None)
        .expect_err(&format!(
            "{phase}: action unexpectedly completed without a human"
        ));
    assert!(
        matches!(
            error.downcast_ref::<ActionError>(),
            Some(ActionError::HumanApprovalRequired { .. })
        ),
        "{error:#}"
    );
    Ok(key)
}

fn click_to(page: &PageSession, url: &str) -> anyhow::Result<()> {
    let previous_document = page.evaluate_script_json("performance.timeOrigin")?;
    page.evaluate_script(&format!(
        "document.getElementById('next').href = {}; document.getElementById('next').click()",
        serde_json::to_string(url)?
    ))?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        if page.current_url().ok().as_deref() == Some(url)
            && matches!(page.evaluate_script_json("performance.timeOrigin"), Ok(current) if current != previous_document)
            && button(page).is_ok()
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    anyhow::bail!("fixture click navigation did not finish")
}

#[test]
fn page_action_grants_do_not_cross_origins_with_identical_stable_keys() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let first = Fixture::new(false)?;
    let second = Fixture::new(false)?;
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    for scope in [
        ApprovalScope::Timeboxed { ms: 60_000 },
        ApprovalScope::ActionOnly,
    ] {
        page.navigate(&first.url)?;
        page.set_policy_rules(vec![rule(scope)])?;
        let old_key = require_approval(&page, "initial page A")?;
        page.approve_pending_policy_action()?;
        click_to(&page, &second.url)?;
        let (_, new_key) = button(&page)?;
        assert_eq!(
            old_key, new_key,
            "the target identity must be identical across origins"
        );
        require_approval(&page, "new origin B with the same stable key")?;
        assert!(page
            .evaluate_script("document.body.dataset.deleted")?
            .value
            .is_none());
    }
    Ok(())
}

#[test]
fn pending_page_a_request_cannot_be_approved_on_page_b() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let first = Fixture::new(false)?;
    let second = Fixture::new(false)?;
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    page.navigate(&first.url)?;
    page.set_policy_rules(vec![rule(ApprovalScope::Timeboxed { ms: 60_000 })])?;
    require_approval(&page, "initial page A pending request")?;
    let pending = page
        .pending_policy_approval_snapshot()
        .expect("pending page A request");
    click_to(&page, &second.url)?;
    page.clear_audit_events();
    assert!(page
        .approve_pending_policy_request_if_current(pending.id, "test")
        .is_err());
    assert!(!page.audit_events().iter().any(|event| matches!(event,
        AuditEvent::HitlEvent { event_type, .. } if event_type == "approved")));
    require_approval(&page, "page B after rejecting the stale request")?;
    Ok(())
}

#[test]
fn same_document_path_or_query_changes_expire_timeboxed_approvals() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let fixture = Fixture::new(false)?;
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    for target in ["/other", "/start?changed=true"] {
        page.navigate(&fixture.url)?;
        page.set_policy_rules(vec![rule(ApprovalScope::Timeboxed { ms: 60_000 })])?;
        require_approval(&page, "initial same-document grant")?;
        page.approve_pending_policy_action()?;
        page.evaluate_script(&format!(
            "history.replaceState(null, '', {})",
            serde_json::to_string(target)?
        ))?;
        require_approval(&page, "after same-document path/query change")?;
    }
    Ok(())
}

#[test]
fn timeboxed_reuses_the_same_page_and_fragment_while_until_navigation_is_stricter(
) -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let fixture = Fixture::new(false)?;
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    for scope in [
        ApprovalScope::Timeboxed { ms: 60_000 },
        ApprovalScope::UntilNavigation,
    ] {
        page.navigate(&fixture.url)?;
        page.set_policy_rules(vec![rule(scope)])?;
        require_approval(&page, "initial fragment control")?;
        page.approve_pending_policy_action()?;
        for _ in 0..2 {
            let (id, key) = button(&page)?;
            page.act(Some(id), Some(&key), "click", None)?;
        }
        page.evaluate_script("history.replaceState(null, '', '#changed')")?;
        if scope == ApprovalScope::UntilNavigation {
            require_approval(&page, "until_navigation fragment change")?;
        } else {
            let (id, key) = button(&page)?;
            page.act(Some(id), Some(&key), "click", None)?;
        }
    }
    Ok(())
}

#[test]
fn reload_and_a_b_a_navigation_expire_timeboxed_grants_at_the_original_url() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let first = Fixture::new(false)?;
    let second = Fixture::new(false)?;
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    for reload in [true, false] {
        page.navigate(&first.url)?;
        page.set_policy_rules(vec![rule(ApprovalScope::Timeboxed { ms: 60_000 })])?;
        let old_key = require_approval(&page, "initial reload/ABA grant")?;
        page.approve_pending_policy_action()?;
        if reload {
            click_to(&page, &first.url)?;
        } else {
            click_to(&page, &second.url)?;
            click_to(&page, &first.url)?;
        }
        assert_eq!(button(&page)?.1, old_key);
        require_approval(&page, "new document at original URL")?;
    }
    Ok(())
}

#[test]
fn navigation_capability_retries_across_an_intermediate_script_document() -> anyhow::Result<()> {
    navigation_capability_retry(ApprovalScope::ActionOnly)
}

#[test]
fn navigation_capability_until_navigation_retries_across_an_intermediate_script_document(
) -> anyhow::Result<()> {
    navigation_capability_retry(ApprovalScope::UntilNavigation)
}

#[test]
fn navigation_capability_timeboxed_retries_across_an_intermediate_script_document(
) -> anyhow::Result<()> {
    navigation_capability_retry(ApprovalScope::Timeboxed { ms: 60_000 })
}

fn navigation_capability_retry(scope: ApprovalScope) -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let fixture = Fixture::new(true)?;
    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    let mut navigation_rule = rule(scope);
    navigation_rule.role = None;
    navigation_rule.path_prefix = Some("/approved".to_string());
    page.set_policy_rules(vec![navigation_rule])?;
    let first = page.navigate_public(&fixture.url, true);
    let started = Instant::now();
    while page.pending_policy_approval_snapshot().is_none()
        && started.elapsed() < Duration::from_secs(10)
    {
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        page.pending_policy_approval_snapshot()
            .unwrap_or_else(|| {
                let result = match &first {
                    Err(error) => format!("Err({error:#})"),
                    Ok(url) => format!("Ok({url:?})"),
                };
                panic!("script redirect must require approval; first navigation result: {result}");
            })
            .request
            .scope,
        scope
    );
    let error = match first {
        Err(error) => error,
        Ok(_) => page
            .check_navigation_rejection()
            .expect_err("late script navigation rejection"),
    };
    assert!(matches!(
        error.downcast_ref::<ActionError>(),
        Some(ActionError::HumanApprovalRequired { .. })
    ));
    eprintln!(
        "navigation retry: scope={scope:?}, source={:?}, requested={:?}",
        page.current_url()?,
        fixture.url
    );
    page.approve_pending_policy_action()?;
    let final_url = page
        .navigate_public(&fixture.url, true)
        .with_context(|| format!("approved retry for {scope:?}"))?;
    assert!(
        final_url.ends_with("/approved"),
        "approved retry must reach its destination"
    );
    Ok(())
}
