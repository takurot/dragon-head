//! Speculative State Generation Pipeline (PR-20 / ISSUE-10).
//!
//! Predicts the AI's next action from session history and portable
//! domain-pack hints, pre-stages the resulting SRE state for near-zero-TTFT
//! responses, and falls back to full-state resend with a replay snapshot when
//! the prediction misses (`StateDelta::Mismatch`).
//!
//! The engine *fronts* [`crate::sre::pipeline::AsyncPipeline`] rather than
//! modifying it: callers check [`SpeculativeEngine::pre_generate`] first — a
//! cache hit bypasses `submit_state` entirely (near-zero TTFT); a miss falls
//! through to the normal pipeline flow, whose result is then fed back via
//! [`SpeculativeEngine::observe_state`] / [`SpeculativeEngine::record_transition`]
//! / [`SpeculativeEngine::verify`].
mod codec;
mod model;

pub use codec::{decode_model, encode_model, SpeculativeCodecError};
pub use model::{ActionSignature, DomainHint, TransitionModel};

use crate::sre::state::SemanticState;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

/// Bound on `mismatch_log` size — oldest entries are dropped at capacity so
/// replay/debug data cannot grow unbounded over a long session.
const MISMATCH_LOG_CAPACITY: usize = 32;

/// Bound on `snapshot_cache` size — oldest-observed snapshots are evicted at
/// capacity so a long browsing session with many distinct page hashes cannot
/// keep every DOM snapshot alive indefinitely.
const SNAPSHOT_CACHE_CAPACITY: usize = 64;

/// A speculative guess at the AI's next action and the state it will produce.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeculativePrediction {
    pub predicted_action: ActionSignature,
    /// `None` when the action was predicted but its resulting state hash has
    /// never been observed in this session (state hashes are page-instance
    /// specific and cannot be guessed for unseen transitions).
    pub predicted_state_hash: Option<String>,
    /// Frequency-ratio confidence of the action prediction, in `0.0..=1.0`.
    pub confidence: f64,
}

/// Replay/debug record captured when a prediction misses, so the mismatch can
/// be inspected or replayed later.
#[derive(Debug, Clone, PartialEq)]
pub struct MismatchSnapshot {
    pub current_state_hash: String,
    pub predicted_action: ActionSignature,
    pub recorded_at: u64,
}

/// Outcome of comparing a [`SpeculativePrediction`] against the actual
/// resulting [`SemanticState`] — the "backtracking" signal ISSUE-10 calls for.
#[derive(Debug, Clone, PartialEq)]
pub enum StateDelta {
    /// The predicted state hash matches the actual one — the pre-generated
    /// snapshot can be served as-is.
    Match { state_hash: String },
    /// The prediction missed: caller must fall back to a full-state resend.
    /// `snapshot` is also appended to the engine's bounded `mismatch_log`.
    Mismatch {
        predicted_state_hash: String,
        actual_state_hash: String,
        snapshot: MismatchSnapshot,
    },
}

struct Inner {
    model: TransitionModel,
    domain_hints: Vec<DomainHint>,
    /// Observed states keyed by `state_hash`, used to serve `pre_generate`
    /// cache hits without regenerating anything (the near-zero-TTFT path).
    /// Bounded by `SNAPSHOT_CACHE_CAPACITY`; `snapshot_order` tracks
    /// insertion order so the oldest entry can be evicted at capacity.
    snapshot_cache: HashMap<String, Arc<SemanticState>>,
    snapshot_order: VecDeque<String>,
    mismatch_log: VecDeque<MismatchSnapshot>,
    last_action: Option<ActionSignature>,
}

impl Inner {
    fn new(domain_hints: Vec<DomainHint>) -> Self {
        Self {
            model: TransitionModel::new(),
            domain_hints,
            snapshot_cache: HashMap::new(),
            snapshot_order: VecDeque::with_capacity(SNAPSHOT_CACHE_CAPACITY),
            mismatch_log: VecDeque::with_capacity(MISMATCH_LOG_CAPACITY),
            last_action: None,
        }
    }
}

/// Predicts the AI's next action/state and pre-stages cached
/// [`SemanticState`] snapshots for near-zero-TTFT serving (PR-20 / ISSUE-10,
/// Spec §3.5).
///
/// Mirrors [`crate::dom_signature::DOMSignatureCache`]'s
/// `Mutex<..>`-guarded-cache style: shared via `&self` across threads, with
/// all mutable state behind a single mutex.
pub struct SpeculativeEngine {
    inner: Mutex<Inner>,
}

impl SpeculativeEngine {
    pub fn new(domain_hints: Vec<DomainHint>) -> Self {
        Self {
            inner: Mutex::new(Inner::new(domain_hints)),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        match self.inner.lock() {
            Ok(inner) => inner,
            Err(poisoned) => {
                let mut inner = poisoned.into_inner();
                let domain_hints = inner.domain_hints.clone();
                *inner = Inner::new(domain_hints);
                self.inner.clear_poison();
                inner
            }
        }
    }

    /// Drops cached state snapshots and the engine-internal action-sequence
    /// cursor after the underlying page session is replaced (e.g. a browser
    /// restart, ISSUE-149). The cached `SemanticState` snapshots and the
    /// last-observed action belong to the crashed page's DOM (backend node
    /// IDs, `page_instance_id`) and would otherwise be served as if they
    /// were still valid. The learned [`TransitionModel`] is keyed by
    /// content-based state hashes and action signatures, which remain valid
    /// across a restart, so it is preserved.
    pub fn reset_session(&self) {
        let mut inner = self.lock();
        inner.snapshot_cache.clear();
        inner.snapshot_order.clear();
        inner.last_action = None;
    }

    /// Clears the engine-internal action-sequence cursor (`last_action`)
    /// without touching the snapshot cache or learned [`TransitionModel`].
    ///
    /// Used when the caller discards a chained or otherwise un-attributable
    /// action (Spec §3.5 / ISSUE-147 review): without this, the next
    /// [`Self::record_transition`] would link its action to whatever action
    /// preceded the discarded chain, training a false action-sequence edge
    /// between two actions that were never observed back-to-back.
    pub fn clear_action_cursor(&self) {
        self.lock().last_action = None;
    }

    /// Sets the engine-internal action-sequence cursor (`last_action`) to
    /// `action` without recording a transition (Spec §3.5 / ISSUE-147
    /// round-10 review).
    ///
    /// Used when [`Self::record_transition`] is skipped because
    /// `previous_semantic_state` was an unverified speculative snapshot, but
    /// `action` was nonetheless executed and observed by the caller: without
    /// this, the engine's cursor would remain on the action *before* `action`,
    /// so the next [`Self::record_transition`] would link its action to the
    /// wrong predecessor, training a false action-sequence edge.
    pub fn advance_action_cursor(&self, action: &ActionSignature) {
        self.lock().last_action = Some(action.clone());
    }

    /// Cache an observed state by its hash, making it servable via
    /// [`Self::pre_generate`] on a future cache hit. Bounded by
    /// `SNAPSHOT_CACHE_CAPACITY`: the oldest-observed snapshot is evicted
    /// once the cache is full, so a long session cannot grow it unbounded.
    pub fn observe_state(&self, state: Arc<SemanticState>) {
        let mut inner = self.lock();
        let hash = state.state_hash().to_string();
        let is_new = !inner.snapshot_cache.contains_key(&hash);
        inner.snapshot_cache.insert(hash.clone(), state);
        if is_new {
            inner.snapshot_order.push_back(hash);
            if inner.snapshot_order.len() > SNAPSHOT_CACHE_CAPACITY {
                if let Some(oldest) = inner.snapshot_order.pop_front() {
                    inner.snapshot_cache.remove(&oldest);
                }
            }
        }
    }

    /// Record an observed `from_state_hash --action--> to_state_hash`
    /// transition, also linking it to the previously recorded action so the
    /// portable action-sequence table improves.
    pub fn record_transition(
        &self,
        from_state_hash: &str,
        action: &ActionSignature,
        to_state_hash: &str,
    ) {
        let mut inner = self.lock();
        let prior = inner.last_action.clone();
        inner
            .model
            .record(from_state_hash, action, to_state_hash, prior.as_ref());
        inner.last_action = Some(action.clone());
    }

    /// Predict the next action (and, if a matching transition has been
    /// observed in this session, the resulting state hash).
    pub fn predict(
        &self,
        current_state_hash: &str,
        last_action: Option<&ActionSignature>,
    ) -> Option<SpeculativePrediction> {
        let inner = self.lock();
        let (predicted_action, confidence) = inner
            .model
            .predict_action(last_action, &inner.domain_hints)?;
        let predicted_state_hash = inner
            .model
            .predict_state(current_state_hash, &predicted_action)
            .map(|(hash, _)| hash);
        Some(SpeculativePrediction {
            predicted_action,
            predicted_state_hash,
            confidence,
        })
    }

    /// The near-zero-TTFT path: predict, then look up the predicted state
    /// hash in the snapshot cache. Returns `Some` only on a concrete cache
    /// hit — callers fall through to the normal pipeline on `None`.
    ///
    /// The cached snapshot is returned with refreshed `page_instance_id` and
    /// `timestamp` (same `state_hash`/content) so repeat hits don't leak the
    /// stale metadata of the original observation into serialized output.
    pub fn pre_generate(
        &self,
        current_state_hash: &str,
        last_action: Option<&ActionSignature>,
    ) -> Option<Arc<SemanticState>> {
        let inner = self.lock();
        let (predicted_action, _confidence) = inner
            .model
            .predict_action(last_action, &inner.domain_hints)?;
        let (predicted_state_hash, _confidence) = inner
            .model
            .predict_state(current_state_hash, &predicted_action)?;
        let cached = inner.snapshot_cache.get(&predicted_state_hash)?;
        Some(Arc::new(cached.with_refreshed_metadata()))
    }

    /// Compare a prediction against the actual resulting state. A hash
    /// mismatch is captured into a [`MismatchSnapshot`] and appended to the
    /// bounded `mismatch_log` for replay/debugging — this is the
    /// backtracking signal ISSUE-10 calls for; the caller should fall back to
    /// a full-state resend.
    ///
    /// When the prediction made no concrete state guess
    /// (`predicted_state_hash: None`, e.g. a cold-start action-only
    /// prediction), there is nothing to falsify, so this returns `Match`
    /// without recording a mismatch.
    pub fn verify(
        &self,
        prediction: &SpeculativePrediction,
        actual_state: &SemanticState,
    ) -> StateDelta {
        let actual_state_hash = actual_state.state_hash().to_string();
        let Some(predicted_state_hash) = prediction.predicted_state_hash.clone() else {
            return StateDelta::Match {
                state_hash: actual_state_hash,
            };
        };

        if predicted_state_hash == actual_state_hash {
            return StateDelta::Match {
                state_hash: actual_state_hash,
            };
        }

        let snapshot = MismatchSnapshot {
            current_state_hash: actual_state_hash.clone(),
            predicted_action: prediction.predicted_action.clone(),
            recorded_at: now_unix_secs(),
        };
        let mut inner = self.lock();
        if inner.mismatch_log.len() == MISMATCH_LOG_CAPACITY {
            inner.mismatch_log.pop_front();
        }
        inner.mismatch_log.push_back(snapshot.clone());

        StateDelta::Mismatch {
            predicted_state_hash,
            actual_state_hash,
            snapshot,
        }
    }

    /// Corrects the transition model after a speculative-hit mismatch
    /// (Spec §3.5 / ISSUE-147 round-10 review): removes the stale
    /// `from_state_hash --action--> stale_to_state_hash` entry (so the
    /// known-wrong snapshot is not predicted again) and records the
    /// actually-observed `from_state_hash --action--> actual_to_state_hash`
    /// transition, then advances the engine-internal action cursor to
    /// `action` (consistent with [`Self::record_transition`]).
    pub fn correct_transition(
        &self,
        from_state_hash: &str,
        action: &ActionSignature,
        stale_to_state_hash: &str,
        actual_to_state_hash: &str,
    ) {
        let mut inner = self.lock();
        let prior = inner.last_action.clone();
        inner.model.correct_transition(
            from_state_hash,
            action,
            stale_to_state_hash,
            actual_to_state_hash,
            prior.as_ref(),
        );
        inner.last_action = Some(action.clone());
    }

    /// Export the portable action-sequence table as a FlatBuffers byte
    /// buffer, suitable for sharing across sessions as a domain pack.
    pub fn export_model(&self) -> Vec<u8> {
        let inner = self.lock();
        encode_model(&inner.model.action_sequence_records())
    }

    /// Import and merge a FlatBuffers-encoded action-sequence table produced
    /// by [`Self::export_model`] (or another engine's domain pack export).
    pub fn import_model(&self, bytes: &[u8]) -> Result<(), SpeculativeCodecError> {
        let records = decode_model(bytes)?;
        let mut inner = self.lock();
        inner.model.merge_action_sequences(records);
        Ok(())
    }

    /// Snapshot of the bounded mismatch replay/debug log, oldest first.
    pub fn mismatch_log(&self) -> Vec<MismatchSnapshot> {
        self.lock().mismatch_log.iter().cloned().collect()
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sre::{LoadProfile, SemanticNode};

    fn action(name: &str) -> ActionSignature {
        ActionSignature::from(name)
    }

    fn state_with_label(label: &str) -> SemanticState {
        SemanticState::new(
            SemanticNode {
                role: "root".to_string(),
                label: Some(label.to_string()),
                ..Default::default()
            },
            LoadProfile::default(),
        )
    }

    #[test]
    fn predict_returns_none_cold() {
        let engine = SpeculativeEngine::new(vec![]);
        assert_eq!(
            engine.predict("hash_a", Some(&action("click:search"))),
            None
        );
    }

    #[test]
    fn predict_returns_some_after_transitions_recorded() {
        let engine = SpeculativeEngine::new(vec![]);
        let click = action("click:search_button");
        let type_input = action("type:search_input");

        engine.record_transition("hash_a", &click, "hash_b");
        engine.record_transition("hash_b", &type_input, "hash_c");

        let prediction = engine.predict("hash_b", Some(&click)).unwrap();
        assert_eq!(prediction.predicted_action, type_input);
        assert_eq!(prediction.predicted_state_hash, Some("hash_c".to_string()));
        assert_eq!(prediction.confidence, 1.0);
    }

    #[test]
    fn pre_generate_serves_cached_snapshot_on_hit() {
        let engine = SpeculativeEngine::new(vec![]);
        let click = action("click:search_button");
        let type_input = action("type:search_input");

        let next_state = Arc::new(state_with_label("results page"));
        let next_hash = next_state.state_hash().to_string();

        engine.record_transition("hash_a", &click, &next_hash);
        // Link click -> type_input so predict_action resolves to type_input,
        // then predict_state(hash_a, type_input) must also resolve — record
        // the same observed transition under the predicted action too.
        engine.record_transition("hash_a", &type_input, &next_hash);
        engine.record_transition("hash_a", &type_input, &next_hash);
        engine.observe_state(next_state.clone());

        let served = engine.pre_generate("hash_a", Some(&click)).unwrap();
        assert_eq!(served.state_hash(), next_state.state_hash());
        assert_eq!(served.root(), next_state.root());
        assert_ne!(
            served.page_instance_id(),
            next_state.page_instance_id(),
            "served snapshot must carry fresh page-instance metadata, not the stale capture's"
        );
    }

    #[test]
    fn pre_generate_refreshes_metadata_on_repeat_hits() {
        let engine = SpeculativeEngine::new(vec![]);
        let click = action("click:search_button");
        let type_input = action("type:search_input");

        let next_state = Arc::new(state_with_label("results page"));
        let next_hash = next_state.state_hash().to_string();

        engine.record_transition("hash_a", &click, &next_hash);
        engine.record_transition("hash_a", &type_input, &next_hash);
        engine.record_transition("hash_a", &type_input, &next_hash);
        engine.observe_state(next_state.clone());

        let first = engine.pre_generate("hash_a", Some(&click)).unwrap();
        let second = engine.pre_generate("hash_a", Some(&click)).unwrap();

        assert_eq!(first.state_hash(), second.state_hash());
        assert_ne!(
            first.page_instance_id(),
            second.page_instance_id(),
            "each pre_generate hit must mint fresh page-instance metadata"
        );
    }

    #[test]
    fn observe_state_evicts_oldest_snapshot_beyond_capacity() {
        let engine = SpeculativeEngine::new(vec![]);

        let states: Vec<Arc<SemanticState>> = (0..SNAPSHOT_CACHE_CAPACITY + 1)
            .map(|i| Arc::new(state_with_label(&format!("page {i}"))))
            .collect();
        let first_hash = states[0].state_hash().to_string();
        let last_hash = states[states.len() - 1].state_hash().to_string();

        for state in &states {
            engine.observe_state(state.clone());
        }

        let inner = engine.lock();
        assert_eq!(inner.snapshot_cache.len(), SNAPSHOT_CACHE_CAPACITY);
        assert!(
            !inner.snapshot_cache.contains_key(&first_hash),
            "the oldest-observed snapshot should have been evicted"
        );
        assert!(
            inner.snapshot_cache.contains_key(&last_hash),
            "the most recently observed snapshot must remain cached"
        );
    }

    #[test]
    fn pre_generate_misses_when_predicted_state_was_never_observed() {
        let engine = SpeculativeEngine::new(vec![]);
        let click = action("click:search_button");
        let type_input = action("type:search_input");

        engine.record_transition("hash_a", &click, "hash_b");
        engine.record_transition("hash_b", &type_input, "hash_c");
        // Note: never call `observe_state` for "hash_c".

        assert_eq!(engine.pre_generate("hash_b", Some(&click)), None);
    }

    #[test]
    fn verify_returns_match_on_hash_agreement() {
        let engine = SpeculativeEngine::new(vec![]);
        let actual = state_with_label("results page");
        let prediction = SpeculativePrediction {
            predicted_action: action("type:search_input"),
            predicted_state_hash: Some(actual.state_hash().to_string()),
            confidence: 0.9,
        };

        let delta = engine.verify(&prediction, &actual);
        assert_eq!(
            delta,
            StateDelta::Match {
                state_hash: actual.state_hash().to_string()
            }
        );
        assert!(engine.mismatch_log().is_empty());
    }

    #[test]
    fn verify_returns_mismatch_with_snapshot_and_appends_to_log() {
        let engine = SpeculativeEngine::new(vec![]);
        let actual = state_with_label("error page");
        let predicted_action = action("type:search_input");
        let prediction = SpeculativePrediction {
            predicted_action: predicted_action.clone(),
            predicted_state_hash: Some("predicted_hash_that_never_occurred".to_string()),
            confidence: 0.8,
        };

        let delta = engine.verify(&prediction, &actual);
        let actual_hash = actual.state_hash().to_string();
        match &delta {
            StateDelta::Mismatch {
                predicted_state_hash,
                actual_state_hash,
                snapshot,
            } => {
                assert_eq!(predicted_state_hash, "predicted_hash_that_never_occurred");
                assert_eq!(actual_state_hash, &actual_hash);
                assert_eq!(snapshot.current_state_hash, actual_hash);
                assert_eq!(snapshot.predicted_action, predicted_action);
                assert!(snapshot.recorded_at > 0);
            }
            other => panic!("expected Mismatch, got {other:?}"),
        }

        let log = engine.mismatch_log();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].current_state_hash, actual_hash);
        assert_eq!(log[0].predicted_action, predicted_action);
    }

    #[test]
    fn verify_returns_match_when_no_concrete_state_was_predicted() {
        let engine = SpeculativeEngine::new(vec![]);
        let actual = state_with_label("results page");
        let prediction = SpeculativePrediction {
            predicted_action: action("type:search_input"),
            predicted_state_hash: None,
            confidence: 0.5,
        };

        let delta = engine.verify(&prediction, &actual);
        assert_eq!(
            delta,
            StateDelta::Match {
                state_hash: actual.state_hash().to_string()
            }
        );
        assert!(engine.mismatch_log().is_empty());
    }

    #[test]
    fn mismatch_log_drops_oldest_entries_beyond_capacity() {
        let engine = SpeculativeEngine::new(vec![]);
        let predicted_action = action("type:search_input");

        for i in 0..(MISMATCH_LOG_CAPACITY + 5) {
            let actual = state_with_label(&format!("page {i}"));
            let prediction = SpeculativePrediction {
                predicted_action: predicted_action.clone(),
                predicted_state_hash: Some(format!("never_observed_{i}")),
                confidence: 0.5,
            };
            engine.verify(&prediction, &actual);
        }

        let log = engine.mismatch_log();
        assert_eq!(log.len(), MISMATCH_LOG_CAPACITY);
    }

    #[test]
    fn export_and_import_model_round_trips_through_engine() {
        let source = SpeculativeEngine::new(vec![]);
        let click = action("click:search_button");
        let type_input = action("type:search_input");
        source.record_transition("hash_a", &click, "hash_b");
        source.record_transition("hash_b", &type_input, "hash_c");

        let bytes = source.export_model();

        let imported = SpeculativeEngine::new(vec![]);
        imported.import_model(&bytes).unwrap();

        let prediction = imported.predict("hash_b", Some(&click)).unwrap();
        assert_eq!(prediction.predicted_action, type_input);
        assert_eq!(prediction.confidence, 1.0);
    }

    #[test]
    fn correct_transition_stops_serving_stale_snapshot_and_serves_actual_one() {
        let engine = SpeculativeEngine::new(vec![]);
        let click = action("click:search_button");

        // The engine learned (incorrectly) that hash_a --click--> hash_stale.
        engine.record_transition("hash_a", &click, "hash_stale");

        engine.correct_transition("hash_a", &click, "hash_stale", "hash_actual");

        let inner = engine.lock();
        assert_eq!(
            inner.model.predict_state("hash_a", &click),
            Some(("hash_actual".to_string(), 1.0))
        );
    }

    #[test]
    fn correct_transition_advances_action_cursor() {
        let engine = SpeculativeEngine::new(vec![]);
        let click = action("click:search_button");
        let type_input = action("type:search_input");

        engine.correct_transition("hash_a", &click, "hash_stale", "hash_actual");

        // The cursor now points at `click`, so the next recorded transition
        // links `click -> type_input` in the portable action-sequence table.
        engine.record_transition("hash_actual", &type_input, "hash_b");

        let prediction = engine.predict("hash_actual", Some(&click)).unwrap();
        assert_eq!(prediction.predicted_action, type_input);
    }

    #[test]
    fn advance_action_cursor_sets_prior_action_for_next_transition() {
        let engine = SpeculativeEngine::new(vec![]);
        let click = action("click:search_button");
        let type_input = action("type:search_input");

        engine.advance_action_cursor(&click);
        engine.record_transition("hash_a", &type_input, "hash_b");

        let prediction = engine.predict("hash_a", Some(&click)).unwrap();
        assert_eq!(prediction.predicted_action, type_input);
    }

    #[test]
    fn import_model_rejects_malformed_bytes() {
        let engine = SpeculativeEngine::new(vec![]);
        let err = engine.import_model(&[0xFF, 0x00]).unwrap_err();
        assert!(matches!(err, SpeculativeCodecError::InvalidFlatbuffer(_)));
    }

    #[test]
    fn poisoned_engine_resets_to_cold_state_and_relearns() {
        let hint_from = action("click:domain_navigation");
        let hint_next = action("type:hinted_search");
        let engine = SpeculativeEngine::new(vec![DomainHint {
            from_action: hint_from.clone(),
            predicted_next_action: hint_next.clone(),
            weight: 1.0,
        }]);
        let click = action("click:search_button");
        let type_input = action("type:search_input");
        let next_state = Arc::new(state_with_label("stale predicted page"));
        let next_hash = next_state.state_hash().to_string();

        engine.record_transition("hash_a", &click, &next_hash);
        engine.record_transition("hash_a", &type_input, &next_hash);
        engine.record_transition("hash_a", &type_input, &next_hash);
        engine.observe_state(next_state);
        assert!(engine.pre_generate("hash_a", Some(&click)).is_some());

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = engine.inner.lock().unwrap();
            panic!("poison speculative engine state");
        }));
        assert!(panic.is_err());
        assert!(engine.inner.is_poisoned());

        assert_eq!(engine.pre_generate("hash_a", Some(&click)), None);
        assert!(!engine.inner.is_poisoned());
        assert_eq!(engine.predict("hash_a", Some(&click)), None);
        let hinted = engine.predict("unknown_hash", Some(&hint_from)).unwrap();
        assert_eq!(hinted.predicted_action, hint_next);
        assert_eq!(hinted.predicted_state_hash, None);

        engine.record_transition("hash_a", &click, "hash_b");
        engine.record_transition("hash_b", &type_input, "hash_c");
        let prediction = engine.predict("hash_b", Some(&click)).unwrap();
        assert_eq!(prediction.predicted_action, type_input);
        assert_eq!(prediction.predicted_state_hash, Some("hash_c".to_string()));
    }
}
