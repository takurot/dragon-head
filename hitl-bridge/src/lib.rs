//! Reference Slack/Teams HITL (Human-In-The-Loop) approval bridge.
//!
//! Polls a `core_runtime::PageSession` for pending policy-approval requests,
//! relays them to a chat tool, and resolves the human's decision back into the
//! session — enforcing a session-level exclusive lock so two reviewers cannot
//! both act on the same request, and recording an immutable audit trail of
//! who decided what, when, and against which Outcome Projection (spec ACT-05).
//!
//! See `docs/hitl-slack-bridge.md` for setup and message-format details.

pub mod audit;
pub mod bridge;
pub mod gateway;
pub mod lock;
pub mod notifier;
pub mod server;
