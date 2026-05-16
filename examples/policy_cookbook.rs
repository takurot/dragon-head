//! Dragon Head — Policy Cookbook
//!
//! Demonstrates how to compose PolicyRules for common enterprise scenarios:
//!   1. Block navigation to restricted domains
//!   2. Require human approval for financial transactions
//!   3. Require approval on a specific checkout path (time-boxed scope)
//!   4. Load rules from an embedded JSON string (same format as the file on disk)
//!
//! Run:
//!   cargo run --example policy_cookbook
//!
//! No external credentials or running Chrome required.

use core_runtime::policy::{
    ApprovalScope, PolicyAction, PolicyContext, PolicyDecision, PolicyEngine, PolicyRule,
};

fn main() {
    println!("=== Dragon Head Policy Cookbook ===\n");

    // ──────────────────────────────────────────────
    // Recipe 1: Block a domain outright
    // ──────────────────────────────────────────────
    demo_block_domain();

    // ──────────────────────────────────────────────
    // Recipe 2: Require human approval for financial buttons
    // ──────────────────────────────────────────────
    demo_require_approval_financial();

    // ──────────────────────────────────────────────
    // Recipe 3: Require time-boxed approval on a path
    // ──────────────────────────────────────────────
    demo_timebox_approval_on_path();

    // ──────────────────────────────────────────────
    // Recipe 4: Load rules from JSON (same as sample_policy.json)
    // ──────────────────────────────────────────────
    demo_load_from_json();

    println!("=== Done ===");
}

// ─────────────────────────────────────────────────────────
// Recipe 1
// ─────────────────────────────────────────────────────────

fn demo_block_domain() {
    println!("[Recipe 1] Block navigation to a restricted domain");

    let engine = PolicyEngine::try_new(vec![PolicyRule {
        id: "block-social-media".to_string(),
        domain: Some("social.example.com".to_string()),
        path_prefix: None,
        role: None,
        text_regex: None,
        context_regex: None,
        action: PolicyAction::Block,
        scope: None,
        outcome_projector: None,
    }])
    .expect("valid rules");

    let cases = [
        ("https://social.example.com/feed", PolicyAction::Block),
        ("https://work.example.com/dashboard", PolicyAction::Allow),
    ];

    for (url, expected) in cases {
        let decision = engine.evaluate(&ctx(url, "navigate", None, None, None));
        let marker = if decision.action == expected {
            "OK"
        } else {
            "FAIL"
        };
        println!("  [{marker}] {url} → {:?}", decision.action);
    }
    println!();
}

// ─────────────────────────────────────────────────────────
// Recipe 2
// ─────────────────────────────────────────────────────────

fn demo_require_approval_financial() {
    println!("[Recipe 2] Require human approval for financial buttons");

    let engine = PolicyEngine::try_new(vec![PolicyRule {
        id: "require-approval-purchase-buttons".to_string(),
        domain: None,
        path_prefix: None,
        role: Some("button".to_string()),
        text_regex: Some(r"(?i)pay|purchase|confirm order".to_string()),
        context_regex: Some(r"(?i)total\s*:|amount\s*:|\$\s*[0-9]".to_string()),
        action: PolicyAction::RequireHumanApproval,
        scope: Some(ApprovalScope::ActionOnly),
        outcome_projector: None,
    }])
    .expect("valid rules");

    // Should trigger approval — button text matches and surrounding context has price
    let financial_ctx = ctx(
        "https://shop.example.com/cart",
        "click",
        Some("button"),
        Some("Pay Now"),
        Some("Total: $49.99"),
    );

    // Should be allowed — no price context
    let safe_ctx = ctx(
        "https://shop.example.com/cart",
        "click",
        Some("button"),
        Some("Pay Now"),
        None, // no surrounding text with price
    );

    print_decision(
        "financial button + price context",
        engine.evaluate(&financial_ctx),
    );
    print_decision(
        "financial button, no price context",
        engine.evaluate(&safe_ctx),
    );
    println!();
}

// ─────────────────────────────────────────────────────────
// Recipe 3
// ─────────────────────────────────────────────────────────

fn demo_timebox_approval_on_path() {
    println!("[Recipe 3] Time-boxed approval on /checkout path");

    let engine = PolicyEngine::try_new(vec![PolicyRule {
        id: "timebox-approval-checkout".to_string(),
        domain: Some("payments.example.com".to_string()),
        path_prefix: Some("/checkout".to_string()),
        role: Some("button".to_string()),
        text_regex: Some(r"(?i)submit|confirm|pay".to_string()),
        context_regex: None,
        action: PolicyAction::RequireHumanApproval,
        // Approval is valid for 5 minutes (300 000 ms)
        scope: Some(ApprovalScope::Timeboxed { ms: 300_000 }),
        outcome_projector: None,
    }])
    .expect("valid rules");

    let in_scope = ctx(
        "https://payments.example.com/checkout/step-3",
        "click",
        Some("button"),
        Some("Submit Payment"),
        None,
    );
    let out_of_scope = ctx(
        "https://payments.example.com/account",
        "click",
        Some("button"),
        Some("Submit"),
        None,
    );

    print_decision(
        "payments.example.com /checkout → Submit",
        engine.evaluate(&in_scope),
    );
    print_decision(
        "payments.example.com /account  → Submit",
        engine.evaluate(&out_of_scope),
    );
    println!();
}

// ─────────────────────────────────────────────────────────
// Recipe 4
// ─────────────────────────────────────────────────────────

fn demo_load_from_json() {
    println!("[Recipe 4] Load policy rules from JSON (mirrors sample_policy.json)");

    // This is the same JSON format used in examples/sample_policy.json and
    // core-runtime/policy/default_rules.json.
    let json = r#"[
        {
            "id": "block-account-destruction",
            "role": "button",
            "text_regex": "(?i)delete\\s+account|remove\\s+account|close\\s+account",
            "context_regex": "(?i)account|profile",
            "action": "block"
        },
        {
            "id": "require-approval-financial-action",
            "role": "button",
            "text_regex": "(?i)pay|purchase|transfer|submit\\s+order|confirm",
            "context_regex": "(?i)total\\s*:|amount\\s*:|\\$\\s*[0-9]",
            "action": "require_human_approval",
            "scope": { "type": "action_only" }
        }
    ]"#;

    let engine = PolicyEngine::try_from_json_str(json).expect("valid JSON rules");
    println!("  Loaded {} rules from JSON string", engine.rules().len());

    let delete_account_ctx = ctx(
        "https://app.example.com/settings",
        "click",
        Some("button"),
        Some("Delete Account"),
        Some("Manage your account and profile settings"),
    );
    print_decision(
        "'Delete Account' button in account context",
        engine.evaluate(&delete_account_ctx),
    );
    println!();
}

// ─────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────

fn ctx(
    url: &str,
    action: &str,
    role: Option<&str>,
    text: Option<&str>,
    surrounding: Option<&str>,
) -> PolicyContext {
    PolicyContext {
        url: url.to_string(),
        action: action.to_string(),
        target_role: role.map(str::to_string),
        target_text: text.map(str::to_string),
        surrounding_text: surrounding.map(str::to_string),
    }
}

fn print_decision(label: &str, decision: PolicyDecision) {
    let rule = decision.rule_id.as_deref().unwrap_or("(default allow)");
    println!("  {label}");
    println!("    → action={:?}  rule={rule}", decision.action);
}
