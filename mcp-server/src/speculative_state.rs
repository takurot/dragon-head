use super::*;

/// Outcome of [`resolve_speculative_state`] — distinguishes a verified
/// pre-generation hit from the various reasons a `get_state` call falls back
/// to a full capture (Spec §3.5 / ISSUE-147).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpeculativeOutcome {
    /// A verified pre-generated snapshot is available and can be served.
    Hit,
    /// `force_refresh` was requested; speculation is bypassed entirely.
    MissForceRefresh,
    /// The previous `get_state` served a speculative hit whose prediction
    /// has not yet been checked against a real DOM capture. Speculation is
    /// bypassed for this call to force a real capture, reconciling
    /// `previous_semantic_state` with the live page before another
    /// speculative hit can be served from it (Spec §3.5 / ISSUE-147
    /// round-8 review).
    MissUnverifiedPriorHit,
    /// There is no prior state or no action pending since the last
    /// `get_state`, so there is nothing to predict from.
    MissNoPendingAction,
    /// The engine has not yet learned a next-action prediction for the
    /// current state.
    MissNoPrediction,
    /// The predicted next action does not match the action that was actually
    /// executed.
    MissPredictionMismatch,
    /// The predicted action matched, but no cached snapshot exists for the
    /// predicted resulting state.
    MissPreGenerateNone,
}

/// Pure decision function for the speculative `get_state` fast path
/// (Spec §3.5 / ISSUE-147). Given the engine and the backend's tracked
/// action/state history, determines whether a pre-generated snapshot can be
/// served, and if not, why not.
///
/// `current_state_hash` (the state the AI is currently at) is
/// `previous_state.state_hash()`; `last_action` is the action that led to
/// that state. A hit additionally requires that the engine's predicted next
/// action matches `pending_action` (the action just executed via `act`).
pub(crate) fn resolve_speculative_state(
    speculative: &SpeculativeEngine,
    previous_state: Option<&SemanticState>,
    last_action: Option<&ActionSignature>,
    pending_action: Option<&ActionSignature>,
    force_refresh: bool,
    prior_hit_unverified: bool,
) -> (
    Option<Arc<SemanticState>>,
    Option<SpeculativePrediction>,
    SpeculativeOutcome,
) {
    if force_refresh {
        return (None, None, SpeculativeOutcome::MissForceRefresh);
    }

    if prior_hit_unverified {
        return (None, None, SpeculativeOutcome::MissUnverifiedPriorHit);
    }

    let (Some(previous_state), Some(pending_action)) = (previous_state, pending_action) else {
        return (None, None, SpeculativeOutcome::MissNoPendingAction);
    };

    let current_state_hash = previous_state.state_hash();
    let Some(prediction) = speculative.predict(current_state_hash, last_action) else {
        return (None, None, SpeculativeOutcome::MissNoPrediction);
    };

    if prediction.predicted_action != *pending_action {
        return (
            None,
            Some(prediction),
            SpeculativeOutcome::MissPredictionMismatch,
        );
    }

    match speculative.pre_generate(current_state_hash, last_action) {
        Some(snapshot) => (Some(snapshot), Some(prediction), SpeculativeOutcome::Hit),
        None => (
            None,
            Some(prediction),
            SpeculativeOutcome::MissPreGenerateNone,
        ),
    }
}

/// Whether [`CoreRuntimeBackend::record_speculative_observation`] should
/// record a `previous_semantic_state -> state` transition under
/// `pending_action` (Spec §3.5 / ISSUE-147 round-9 review).
///
/// A transition is only recorded when `pending_action` was executed since
/// the last capture AND `previous_state` itself reflects a verified (real)
/// DOM capture rather than an unverified speculative snapshot served by the
/// `Hit` branch. Training a transition from an unverified snapshot risks
/// recording an edge that does not exist in the live page's transition
/// graph if that snapshot turns out to have been stale.
pub(crate) fn should_record_transition(
    previous_state_verified: bool,
    previous_state: Option<&SemanticState>,
    pending_action: Option<&ActionSignature>,
) -> bool {
    previous_state_verified && previous_state.is_some() && pending_action.is_some()
}

/// Verifies the most recently served speculative hit against a real DOM
/// capture and corrects the transition model when the cached snapshot was
/// stale.
pub(crate) fn reconcile_served_prediction(
    speculative: &SpeculativeEngine,
    last_served_prediction: &mut Option<SpeculativePrediction>,
    last_served_prediction_source_hash: &mut Option<String>,
    pending_action: Option<&ActionSignature>,
    action_chain_broken: bool,
    state: &SemanticState,
) {
    let Some(served) = last_served_prediction.take() else {
        return;
    };
    let source_hash = last_served_prediction_source_hash.take();
    if pending_action.is_some() || action_chain_broken {
        return;
    }

    let delta = speculative.verify(&served, state);
    if let (
        StateDelta::Mismatch {
            predicted_state_hash,
            actual_state_hash,
            ..
        },
        Some(from_hash),
    ) = (&delta, &source_hash)
    {
        speculative.correct_transition(
            from_hash,
            &served.predicted_action,
            predicted_state_hash,
            actual_state_hash,
        );
    }
}
