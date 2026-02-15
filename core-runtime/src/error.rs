use thiserror::Error;

#[derive(Error, Debug)]
pub enum ActionError {
    #[error("Action failed: target_id and stable_key both failed. Verification required.")]
    VerifyRequired,
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
