use thiserror::Error;

use crate::policy::{ApprovalScope, OutcomeProjection};

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
        /// Guardian Angel: structured outcome projection for the pending action.
        outcome: Option<OutcomeProjection>,
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

/// Errors surfaced when the underlying Chrome process disconnects mid-session
/// (ISSUE-149: Chrome crash/disconnect recovery).
#[derive(Error, Debug)]
pub enum SessionError {
    /// The Chrome process disconnected and the session was automatically
    /// restarted with a fresh page. Navigation state, in-page cookies, and
    /// unsaved form data were reset, and any pending or previously granted
    /// human-approval requests were discarded.
    #[error(
        "Chrome process disconnected; the session was automatically restarted (restart #{restart_count}). \
         Navigation state, in-page cookies, and unsaved form data were reset, and any pending or \
         previously granted human-approval requests were discarded — retry the request and \
         re-request approval if needed."
    )]
    BrowserRestarted { restart_count: u64 },

    /// The Chrome process disconnected and automatic restart could not recover the session.
    #[error("Chrome process disconnected and automatic restart failed: {reason}")]
    BrowserRestartFailed { reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_restarted_message_includes_restart_count_and_reset_notice() {
        let err = SessionError::BrowserRestarted { restart_count: 3 };
        let message = err.to_string();
        assert!(message.contains("restart #3"));
        assert!(message.contains("automatically restarted"));
        assert!(message.contains("retry the request"));
    }

    #[test]
    fn browser_restart_failed_message_includes_reason() {
        let err = SessionError::BrowserRestartFailed {
            reason: "no managed BrowserClient".to_string(),
        };
        let message = err.to_string();
        assert!(message.contains("automatic restart failed"));
        assert!(message.contains("no managed BrowserClient"));
    }
}
