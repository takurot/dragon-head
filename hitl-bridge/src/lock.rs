//! Session-level exclusive lock preventing two reviewers from both resolving
//! the same approval request (spec ACT-05 "HITL Concurrency & Safety").
//!
//! Exactly one of two concurrent `try_claim` calls for the same request ID
//! wins; the loser learns who won and what they decided so the bridge can
//! show them an "already resolved by @X" message instead of silently
//! double-applying the decision.

use std::collections::HashMap;
use std::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approved,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Claim {
    decided_by: String,
    decision: Decision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// The caller is the first to resolve this request and may proceed.
    Won,
    /// Someone else already resolved this request first.
    LostTo {
        decided_by: String,
        decision: Decision,
    },
}

/// Registry of resolved approval-request IDs.
///
/// `try_claim` is the only mutator; it atomically checks-and-sets under a
/// single mutex so two threads racing on the same ID can never both win.
#[derive(Default)]
pub struct SessionLockRegistry {
    claims: Mutex<HashMap<Uuid, Claim>>,
}

impl SessionLockRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Attempt to claim exclusive resolution rights for `id`.
    ///
    /// On [`ClaimOutcome::Won`], the caller is the sole resolver and must
    /// proceed to mutate the gateway and write the audit record. On
    /// [`ClaimOutcome::LostTo`], the caller must not mutate anything further —
    /// another reviewer already resolved this request.
    pub fn try_claim(&self, id: Uuid, decided_by: &str, decision: Decision) -> ClaimOutcome {
        let mut claims = self.claims.lock().expect("lock registry mutex poisoned");
        if let Some(existing) = claims.get(&id) {
            return ClaimOutcome::LostTo {
                decided_by: existing.decided_by.clone(),
                decision: existing.decision,
            };
        }
        claims.insert(
            id,
            Claim {
                decided_by: decided_by.to_string(),
                decision,
            },
        );
        ClaimOutcome::Won
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn first_claim_wins_second_loses_to_first() {
        let registry = SessionLockRegistry::new();
        let id = Uuid::new_v4();

        let first = registry.try_claim(id, "alice", Decision::Approved);
        let second = registry.try_claim(id, "bob", Decision::Rejected);

        assert_eq!(first, ClaimOutcome::Won);
        assert_eq!(
            second,
            ClaimOutcome::LostTo {
                decided_by: "alice".to_string(),
                decision: Decision::Approved,
            }
        );
    }

    #[test]
    fn different_ids_claim_independently() {
        let registry = SessionLockRegistry::new();
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();

        assert_eq!(
            registry.try_claim(first_id, "alice", Decision::Approved),
            ClaimOutcome::Won
        );
        assert_eq!(
            registry.try_claim(second_id, "bob", Decision::Rejected),
            ClaimOutcome::Won
        );
    }

    #[test]
    fn concurrent_claims_on_same_id_produce_exactly_one_winner() {
        let registry = Arc::new(SessionLockRegistry::new());
        let id = Uuid::new_v4();

        let handles: Vec<_> = (0..8)
            .map(|i| {
                let registry = Arc::clone(&registry);
                let name = format!("reviewer-{i}");
                thread::spawn(move || registry.try_claim(id, &name, Decision::Approved))
            })
            .collect();

        let outcomes: Vec<ClaimOutcome> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        let wins = outcomes.iter().filter(|o| **o == ClaimOutcome::Won).count();
        let losses = outcomes.len() - wins;

        assert_eq!(wins, 1, "exactly one claimant must win the lock");
        assert_eq!(losses, 7);
    }
}
