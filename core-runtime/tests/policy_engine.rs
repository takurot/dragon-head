use core_runtime::policy::{ApprovalScope, PolicyAction, PolicyContext, PolicyEngine, PolicyRule};

#[test]
fn test_policy_engine_matches_domain_path_role_and_text() {
    let engine = PolicyEngine::new(vec![PolicyRule {
        id: "block-purchase".to_string(),
        domain: Some("checkout.example.com".to_string()),
        path_prefix: Some("/checkout".to_string()),
        role: Some("button".to_string()),
        text_regex: Some("(?i)purchase|buy".to_string()),
        context_regex: None,
        action: PolicyAction::Block,
        scope: None,
    }]);

    let decision = engine.evaluate(&PolicyContext {
        url: "https://checkout.example.com/checkout/review".to_string(),
        action: "click".to_string(),
        target_role: Some("button".to_string()),
        target_text: Some("Purchase".to_string()),
        surrounding_text: Some("Total: $25".to_string()),
    });

    assert_eq!(decision.action, PolicyAction::Block);
    assert_eq!(decision.rule_id.as_deref(), Some("block-purchase"));
}

#[test]
fn test_policy_engine_matches_context_regex_for_human_approval() {
    let engine = PolicyEngine::new(vec![PolicyRule {
        id: "approve-when-amount-visible".to_string(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_string()),
        text_regex: Some("(?i)submit|pay".to_string()),
        context_regex: Some("(?i)total\\s*:\\s*\\$[0-9]+".to_string()),
        action: PolicyAction::RequireHumanApproval,
        scope: Some(ApprovalScope::ActionOnly),
    }]);

    let decision = engine.evaluate(&PolicyContext {
        url: "https://shop.example.com/checkout".to_string(),
        action: "click".to_string(),
        target_role: Some("button".to_string()),
        target_text: Some("Pay now".to_string()),
        surrounding_text: Some("Total: $149".to_string()),
    });

    assert_eq!(decision.action, PolicyAction::RequireHumanApproval);
    assert_eq!(
        decision.scope,
        Some(ApprovalScope::ActionOnly),
        "require_human_approval must carry scope"
    );
    assert_eq!(
        decision.rule_id.as_deref(),
        Some("approve-when-amount-visible")
    );
}

#[test]
fn test_policy_engine_falls_back_to_allow_when_no_rules_match() {
    let engine = PolicyEngine::new(vec![PolicyRule {
        id: "only-delete".to_string(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_string()),
        text_regex: Some("(?i)delete".to_string()),
        context_regex: None,
        action: PolicyAction::Block,
        scope: None,
    }]);

    let decision = engine.evaluate(&PolicyContext {
        url: "https://example.com/profile".to_string(),
        action: "click".to_string(),
        target_role: Some("button".to_string()),
        target_text: Some("Save".to_string()),
        surrounding_text: Some("Profile settings".to_string()),
    });

    assert_eq!(decision.action, PolicyAction::Allow);
    assert_eq!(decision.rule_id, None);
}
