//! Integration test for the Speculative State Generation Pipeline
//! (PR-20 / ISSUE-10, Spec §3.5).
//!
//! Demonstrates the engine "fronting" `AsyncPipeline` (PR-08's SRE Queue):
//! callers check `pre_generate` first — a cache hit serves a pre-staged
//! `SemanticState` near-instantly, bypassing `submit_state` + regeneration
//! entirely. A miss (or a deliberately wrong guess, "Drift Injection")
//! produces `StateDelta::Mismatch` with a replay snapshot, and
//! `record_transition` lets the model self-correct for the next attempt —
//! the backtracking mechanism ISSUE-10 calls for.
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use core_runtime::{
    speculative::{ActionSignature, SpeculativeEngine, SpeculativePrediction, StateDelta},
    sre::{AsyncPipeline, AsyncPipelineConfig, LoadProfile, SemanticNode, SemanticState},
};

/// Exit Criteria (docs/PLAN.md PR-20): "予測的中時に TTFT < 10ms を達成".
const PREDICTION_HIT_TTFT_BUDGET: Duration = Duration::from_millis(10);

fn page_state(label: &str) -> SemanticState {
    SemanticState::new(
        SemanticNode {
            role: "body".to_string(),
            label: Some(label.to_string()),
            ..Default::default()
        },
        LoadProfile::Minimal,
    )
}

/// Regenerates `state` through the full SRE pipeline (submit -> render ->
/// fast/full SRE stages), returning the elapsed wall time. This is the
/// baseline TTFT a caller pays without speculative pre-generation.
fn regenerate_via_pipeline(
    pipeline: &AsyncPipeline,
    state: SemanticState,
) -> anyhow::Result<Duration> {
    let started = Instant::now();
    let handle = pipeline.submit_state(state)?;
    handle.recv_fast(Duration::from_millis(500))?;
    handle.recv_full(Duration::from_millis(500))?;
    Ok(started.elapsed())
}

#[test]
fn pre_generate_serves_cached_state_near_instantly_on_prediction_hit() -> anyhow::Result<()> {
    let pipeline = AsyncPipeline::new(AsyncPipelineConfig::default());
    let engine = SpeculativeEngine::new(vec![]);

    let nav = ActionSignature::from("nav:open_search");
    let click = ActionSignature::from("click:search_button");

    let landing_page = page_state("landing page");
    let search_page = page_state("search page");
    let results_page = page_state("results page");

    // First visit: walk landing -> search -> results, teaching the engine
    // both "after `nav`, `click` usually follows" (action_sequences) and
    // "from `search_page`, `click` leads to `results_page`" (state_transitions).
    engine.record_transition(landing_page.state_hash(), &nav, search_page.state_hash());
    engine.record_transition(search_page.state_hash(), &click, results_page.state_hash());
    engine.observe_state(Arc::new(results_page.clone()));

    // Baseline: regenerating the same page through the normal SRE pipeline —
    // the flow a caller falls through to on a `pre_generate` cache miss.
    let baseline_ttft = regenerate_via_pipeline(&pipeline, results_page.clone())?;

    // Second visit: caller is back on `search_page`, having just taken `nav`
    // to get there. The engine should predict `click` comes next, that it
    // leads to `results_page`, and serve the cached snapshot directly —
    // bypassing `submit_state` and the SRE queue entirely.
    let started = Instant::now();
    let served = engine.pre_generate(search_page.state_hash(), Some(&nav));
    let prediction_hit_ttft = started.elapsed();

    let served = served.expect(
        "expected the engine to serve the cached results-page snapshot on a prediction hit",
    );
    assert_eq!(
        served.state_hash(),
        results_page.state_hash(),
        "served snapshot must carry the predicted page's content/state_hash"
    );
    assert_eq!(served.root(), results_page.root());
    assert_ne!(
        served.page_instance_id(),
        results_page.page_instance_id(),
        "served snapshot must carry fresh page-instance metadata, not the original capture's"
    );
    assert!(
        prediction_hit_ttft <= PREDICTION_HIT_TTFT_BUDGET,
        "prediction-hit TTFT {prediction_hit_ttft:?} exceeded the {PREDICTION_HIT_TTFT_BUDGET:?} exit-criteria budget"
    );
    assert!(
        prediction_hit_ttft < baseline_ttft,
        "prediction-hit TTFT {prediction_hit_ttft:?} should be faster than the baseline pipeline \
         regeneration TTFT {baseline_ttft:?} — pre-generation should bypass the SRE queue entirely"
    );

    Ok(())
}

#[test]
fn drift_injection_triggers_mismatch_backtracking_and_self_correction() {
    let engine = SpeculativeEngine::new(vec![]);

    let nav = ActionSignature::from("nav:open_search");
    let click = ActionSignature::from("click:search_button");

    let landing_page = page_state("landing page");
    let search_page = page_state("search page");
    let results_page = page_state("results page");
    let error_page = page_state("error page"); // the actual outcome — the prediction guesses wrong

    // Teach the engine: landing --nav--> search_page --click--> results_page
    // (observed once — this is the only transition it has ever seen from here).
    engine.record_transition(landing_page.state_hash(), &nav, search_page.state_hash());
    engine.record_transition(search_page.state_hash(), &click, results_page.state_hash());

    let prediction = engine.predict(search_page.state_hash(), Some(&nav)).expect(
        "engine should have a prediction after observing the landing -> search -> results walk",
    );
    assert_eq!(prediction.predicted_action, click);
    assert_eq!(
        prediction.predicted_state_hash,
        Some(results_page.state_hash().to_string())
    );

    // Drift Injection: this time, clicking from `search_page` actually lands
    // on `error_page`, not the predicted `results_page`.
    let delta = engine.verify(&prediction, &error_page);
    let (predicted_state_hash, actual_state_hash, snapshot) = match delta {
        StateDelta::Mismatch {
            predicted_state_hash,
            actual_state_hash,
            snapshot,
        } => (predicted_state_hash, actual_state_hash, snapshot),
        StateDelta::Match { state_hash } => {
            panic!("expected Mismatch (Drift Injection), got Match {{ state_hash: {state_hash} }}")
        }
    };
    assert_eq!(predicted_state_hash, results_page.state_hash());
    assert_eq!(actual_state_hash, error_page.state_hash());
    assert_eq!(snapshot.current_state_hash, error_page.state_hash());
    assert_eq!(snapshot.predicted_action, prediction.predicted_action);
    assert!(
        snapshot.recorded_at > 0,
        "snapshot should carry a real timestamp"
    );

    let log = engine.mismatch_log();
    assert_eq!(
        log.len(),
        1,
        "the mismatch should be appended to the replay/debug log"
    );
    assert_eq!(log[0], snapshot);

    // Backtracking: record what *actually* happened — twice, so the
    // corrected outcome unambiguously dominates the one-off original guess —
    // and let the model self-correct for the next attempt from the same state.
    engine.record_transition(search_page.state_hash(), &click, error_page.state_hash());
    engine.record_transition(search_page.state_hash(), &click, error_page.state_hash());

    let corrected = engine
        .predict(search_page.state_hash(), Some(&nav))
        .expect("engine should still produce a prediction after correction");
    assert_eq!(corrected.predicted_action, click);
    assert_eq!(
        corrected.predicted_state_hash,
        Some(error_page.state_hash().to_string()),
        "after observing the actual outcome twice, the engine should now predict it as the most likely next state"
    );

    // Re-verifying against the (now correctly predicted) actual state matches.
    assert_eq!(
        engine.verify(&corrected, &error_page),
        StateDelta::Match {
            state_hash: error_page.state_hash().to_string()
        }
    );
    assert_eq!(
        engine.mismatch_log().len(),
        1,
        "a correct verification must not append a new mismatch entry"
    );
}

#[test]
fn verify_with_unseen_prediction_reports_match_and_records_no_mismatch() {
    // Sanity check on the API surface used above: a hand-built prediction
    // that never goes through `predict`/`pre_generate` (e.g. one with no
    // concrete state guess) must not be misreported as a Drift.
    let engine = SpeculativeEngine::new(vec![]);
    let actual = page_state("any page");
    let cold_prediction = SpeculativePrediction {
        predicted_action: ActionSignature::from("click:something"),
        predicted_state_hash: None,
        confidence: 0.4,
    };

    assert_eq!(
        engine.verify(&cold_prediction, &actual),
        StateDelta::Match {
            state_hash: actual.state_hash().to_string()
        }
    );
    assert!(engine.mismatch_log().is_empty());
}
