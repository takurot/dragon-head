use core_runtime::policy::{ApprovalScope, PolicyAction, PolicyContext, PolicyEngine, PolicyRule};

#[test]
fn test_policy_engine_matches_domain_path_role_and_text() {
    let engine = PolicyEngine::try_new(vec![PolicyRule {
        id: "block-purchase".to_string(),
        domain: Some("checkout.example.com".to_string()),
        path_prefix: Some("/checkout".to_string()),
        role: Some("button".to_string()),
        text_regex: Some("(?i)purchase|buy".to_string()),
        context_regex: None,
        action: PolicyAction::Block,
        scope: None,
    }])
    .unwrap();

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
    let engine = PolicyEngine::try_new(vec![PolicyRule {
        id: "approve-when-amount-visible".to_string(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_string()),
        text_regex: Some("(?i)submit|pay".to_string()),
        context_regex: Some("(?i)total\\s*:\\s*\\$[0-9]+".to_string()),
        action: PolicyAction::RequireHumanApproval,
        scope: Some(ApprovalScope::ActionOnly),
    }])
    .unwrap();

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
    let engine = PolicyEngine::try_new(vec![PolicyRule {
        id: "only-delete".to_string(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_string()),
        text_regex: Some("(?i)delete".to_string()),
        context_regex: None,
        action: PolicyAction::Block,
        scope: None,
    }])
    .unwrap();

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

/// Regression test for the whitespace path_prefix wildcard bypass.
/// A path_prefix of "   " trims to "" and would match every path via
/// `starts_with("")`. `validate_rule_set` must reject it.
#[test]
fn test_whitespace_path_prefix_is_rejected() {
    let result = PolicyEngine::try_new(vec![PolicyRule {
        id: "whitespace-prefix".to_string(),
        domain: None,
        path_prefix: Some("   ".to_string()),
        role: None,
        text_regex: None,
        context_regex: None,
        action: PolicyAction::Allow,
        scope: None,
    }]);

    assert!(
        result.is_err(),
        "A whitespace-only path_prefix must be rejected by validation"
    );
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("whitespace-only path_prefix"),
        "Error message should mention whitespace-only path_prefix, got: {err}"
    );
}

/// Regression test for fail-closed policy: invalid rules must not silently
/// produce an allow-all engine.
#[test]
fn test_invalid_rules_return_error_not_empty_engine() {
    // A rule with require_human_approval but no scope is invalid.
    let result = PolicyEngine::try_new(vec![PolicyRule {
        id: "bad-rule".to_string(),
        domain: None,
        path_prefix: None,
        role: None,
        text_regex: None,
        context_regex: None,
        action: PolicyAction::RequireHumanApproval,
        scope: None, // missing required scope
    }]);

    assert!(
        result.is_err(),
        "Invalid rules must return Err, not a silent allow-all engine"
    );
}

/// A rule with path_prefix "/login" must match the exact path "/login".
#[test]
fn test_path_prefix_matches_exact_path() {
    let engine = PolicyEngine::try_new(vec![PolicyRule {
        id: "login-rule".to_string(),
        domain: None,
        path_prefix: Some("/login".to_string()),
        role: None,
        text_regex: None,
        context_regex: None,
        action: PolicyAction::Block,
        scope: None,
    }])
    .unwrap();

    let decision = engine.evaluate(&PolicyContext {
        url: "https://example.com/login".to_string(),
        action: "click".to_string(),
        target_role: None,
        target_text: None,
        surrounding_text: None,
    });
    assert_eq!(decision.action, PolicyAction::Block);
}

/// A rule with path_prefix "/login" must match child path "/login/step".
#[test]
fn test_path_prefix_matches_child_path() {
    let engine = PolicyEngine::try_new(vec![PolicyRule {
        id: "login-rule".to_string(),
        domain: None,
        path_prefix: Some("/login".to_string()),
        role: None,
        text_regex: None,
        context_regex: None,
        action: PolicyAction::Block,
        scope: None,
    }])
    .unwrap();

    let decision = engine.evaluate(&PolicyContext {
        url: "https://example.com/login/step".to_string(),
        action: "click".to_string(),
        target_role: None,
        target_text: None,
        surrounding_text: None,
    });
    assert_eq!(decision.action, PolicyAction::Block);
}

/// A rule with path_prefix "/login" must NOT match sibling path "/logina".
#[test]
fn test_path_prefix_rejects_sibling_path() {
    let engine = PolicyEngine::try_new(vec![PolicyRule {
        id: "login-rule".to_string(),
        domain: None,
        path_prefix: Some("/login".to_string()),
        role: None,
        text_regex: None,
        context_regex: None,
        action: PolicyAction::Block,
        scope: None,
    }])
    .unwrap();

    let decision = engine.evaluate(&PolicyContext {
        url: "https://example.com/logina".to_string(),
        action: "click".to_string(),
        target_role: None,
        target_text: None,
        surrounding_text: None,
    });
    assert_eq!(
        decision.action,
        PolicyAction::Allow,
        "/login prefix must NOT match /logina"
    );
}

/// A rule with trailing slash path_prefix "/login/" must match child "/login/step".
#[test]
fn test_path_prefix_trailing_slash_matches_child() {
    let engine = PolicyEngine::try_new(vec![PolicyRule {
        id: "login-rule".to_string(),
        domain: None,
        path_prefix: Some("/login/".to_string()),
        role: None,
        text_regex: None,
        context_regex: None,
        action: PolicyAction::Block,
        scope: None,
    }])
    .unwrap();

    let decision = engine.evaluate(&PolicyContext {
        url: "https://example.com/login/step".to_string(),
        action: "click".to_string(),
        target_role: None,
        target_text: None,
        surrounding_text: None,
    });
    assert_eq!(decision.action, PolicyAction::Block);
}

/// A rule with path_prefix "/foo" must NOT match unrelated sibling "/foobar".
#[test]
fn test_path_prefix_rejects_unrelated_sibling() {
    let engine = PolicyEngine::try_new(vec![PolicyRule {
        id: "foo-rule".to_string(),
        domain: None,
        path_prefix: Some("/foo".to_string()),
        role: None,
        text_regex: None,
        context_regex: None,
        action: PolicyAction::Block,
        scope: None,
    }])
    .unwrap();

    let decision = engine.evaluate(&PolicyContext {
        url: "https://example.com/foobar".to_string(),
        action: "click".to_string(),
        target_role: None,
        target_text: None,
        surrounding_text: None,
    });
    assert_eq!(
        decision.action,
        PolicyAction::Allow,
        "/foo prefix must NOT match /foobar"
    );
}

/// A rule with path_prefix "/login/" must match exact path "/login/".
#[test]
fn test_path_prefix_trailing_slash_exact_match() {
    let engine = PolicyEngine::try_new(vec![PolicyRule {
        id: "login-rule".to_string(),
        domain: None,
        path_prefix: Some("/login/".to_string()),
        role: None,
        text_regex: None,
        context_regex: None,
        action: PolicyAction::Block,
        scope: None,
    }])
    .unwrap();

    let decision = engine.evaluate(&PolicyContext {
        url: "https://example.com/login/".to_string(),
        action: "click".to_string(),
        target_role: None,
        target_text: None,
        surrounding_text: None,
    });
    assert_eq!(decision.action, PolicyAction::Block);
}

/// A rule with path_prefix "/" (root) must match any path.
#[test]
fn test_path_prefix_root_matches_any_path() {
    let engine = PolicyEngine::try_new(vec![PolicyRule {
        id: "root-rule".to_string(),
        domain: None,
        path_prefix: Some("/".to_string()),
        role: None,
        text_regex: None,
        context_regex: None,
        action: PolicyAction::Block,
        scope: None,
    }])
    .unwrap();

    let decision = engine.evaluate(&PolicyContext {
        url: "https://example.com/anything/here".to_string(),
        action: "click".to_string(),
        target_role: None,
        target_text: None,
        surrounding_text: None,
    });
    assert_eq!(decision.action, PolicyAction::Block);
}
