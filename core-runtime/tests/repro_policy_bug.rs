/// Reproduction tests for ISSUE-02: `PolicyEngine` path prefix matching is not segment-aware.
///
/// Before the fix, `CompiledPolicyRule::matches` used a bare `starts_with` check.
/// A rule for `/login` would incorrectly match `/logina` because `"/logina".starts_with("/login")`
/// returns `true`. The fix adds a segment-boundary check: the character immediately following
/// the prefix in the actual path must be `/`, or the path must equal the prefix exactly.
use core_runtime::policy::{PolicyAction, PolicyContext, PolicyEngine, PolicyRule};

fn make_engine(path_prefix: &str) -> PolicyEngine {
    PolicyEngine::try_new(vec![PolicyRule {
        id: "repro-rule".to_string(),
        domain: None,
        path_prefix: Some(path_prefix.to_string()),
        role: None,
        text_regex: None,
        context_regex: None,
        action: PolicyAction::Block,
        scope: None,
    }])
    .expect("valid rule")
}

fn url(path: &str) -> PolicyContext {
    PolicyContext {
        url: format!("https://example.com{path}"),
        action: "click".to_string(),
        target_role: None,
        target_text: None,
        surrounding_text: None,
    }
}

/// Core regression: `/login` rule must NOT match `/logina`.
/// This was the exact failure case reported in ISSUE-02.
#[test]
fn login_prefix_does_not_match_logina() {
    let engine = make_engine("/login");
    let decision = engine.evaluate(&url("/logina"));
    assert_eq!(
        decision.action,
        PolicyAction::Allow,
        "BUG REGRESSION: /login rule must NOT match /logina"
    );
}

/// `/login` rule must NOT match `/loginadmin`.
#[test]
fn login_prefix_does_not_match_loginadmin() {
    let engine = make_engine("/login");
    let decision = engine.evaluate(&url("/loginadmin"));
    assert_eq!(
        decision.action,
        PolicyAction::Allow,
        "/login rule must NOT match /loginadmin"
    );
}

/// `/login` rule must match the exact path `/login`.
#[test]
fn login_prefix_matches_exact() {
    let engine = make_engine("/login");
    let decision = engine.evaluate(&url("/login"));
    assert_eq!(
        decision.action,
        PolicyAction::Block,
        "/login rule must match /login exactly"
    );
}

/// `/login` rule must match child path `/login/step`.
#[test]
fn login_prefix_matches_child_segment() {
    let engine = make_engine("/login");
    let decision = engine.evaluate(&url("/login/step"));
    assert_eq!(
        decision.action,
        PolicyAction::Block,
        "/login rule must match child path /login/step"
    );
}

/// `/login` rule must match deeper nested path `/login/step/two`.
#[test]
fn login_prefix_matches_deep_child() {
    let engine = make_engine("/login");
    let decision = engine.evaluate(&url("/login/step/two"));
    assert_eq!(
        decision.action,
        PolicyAction::Block,
        "/login rule must match deep child /login/step/two"
    );
}

/// An entirely unrelated path must not match.
#[test]
fn login_prefix_does_not_match_unrelated_path() {
    let engine = make_engine("/login");
    let decision = engine.evaluate(&url("/dashboard"));
    assert_eq!(
        decision.action,
        PolicyAction::Allow,
        "/login rule must NOT match unrelated path /dashboard"
    );
}

/// A trailing-slash prefix `/login/` must match child path `/login/step` but NOT `/logina`.
/// Covers the variant of the segment-boundary bug where the prefix itself ends with `/`.
#[test]
fn trailing_slash_prefix_matches_child_but_not_sibling() {
    let engine = make_engine("/login/");
    assert_eq!(
        engine.evaluate(&url("/login/step")).action,
        PolicyAction::Block,
        "/login/ rule must match /login/step"
    );
    assert_eq!(
        engine.evaluate(&url("/logina")).action,
        PolicyAction::Allow,
        "/login/ rule must NOT match /logina"
    );
}

/// A URL with a query string must still match on path alone.
/// Confirms that `Url::parse` path extraction strips the query before comparison.
#[test]
fn login_prefix_matches_url_with_query_string() {
    let engine = make_engine("/login");
    let ctx = PolicyContext {
        url: "https://example.com/login?next=/home".to_string(),
        action: "click".to_string(),
        target_role: None,
        target_text: None,
        surrounding_text: None,
    };
    assert_eq!(
        engine.evaluate(&ctx).action,
        PolicyAction::Block,
        "/login rule must match /login?next=/home (query string must be stripped)"
    );
}

/// A URL with a query string must NOT match when the path does not match.
#[test]
fn login_prefix_does_not_match_logina_with_query_string() {
    let engine = make_engine("/login");
    let ctx = PolicyContext {
        url: "https://example.com/logina?q=1".to_string(),
        action: "click".to_string(),
        target_role: None,
        target_text: None,
        surrounding_text: None,
    };
    assert_eq!(
        engine.evaluate(&ctx).action,
        PolicyAction::Allow,
        "/login rule must NOT match /logina?q=1"
    );
}
