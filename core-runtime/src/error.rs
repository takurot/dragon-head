use thiserror::Error;

use crate::policy::ApprovalScope;

#[derive(Error, Debug)]
pub enum ActionError {
    #[error("Action failed: target_id and stable_key both failed. Verification required.")]
    VerifyRequired,
    #[error("Action blocked by policy rule '{rule_id}'.")]
    Blocked { rule_id: String },
    #[error("Action requires human approval by policy rule '{rule_id}' with scope {scope:?}.")]
    HumanApprovalRequired {
        rule_id: String,
        scope: ApprovalScope,
    },
    /// Self-Healing Context Recovery failed — human must re-identify the target.
    ///
    /// Returned when `target_id`, `stable_key`, and fuzzy-match recovery all
    /// fail for the same element (PR-21 / ISSUE-11 ACT-04 step 4).
    #[error("Self-healing recovery failed; human intervention required: {reason}")]
    AskHumanRequired { reason: String },
}

#[derive(Error, Debug)]
pub enum VerifyError {
    #[error(
        "Verification failed for target_id={target_id}: expected '{expected}', actual '{actual}'"
    )]
    ExpectationMismatch {
        target_id: i64,
        expected: String,
        actual: String,
    },
}

#[derive(Error, Debug)]
pub enum WaitError {
    #[error("Timed out waiting for {operation} after {timeout_ms}ms")]
    Timeout { operation: String, timeout_ms: u64 },
}
