use thiserror::Error;

#[derive(Error, Debug)]
pub enum ActionError {
    #[error("Action failed: target_id and stable_key both failed. Verification required.")]
    VerifyRequired,
}
