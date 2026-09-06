use crate::metrics::{MultiStepResult, RunResult, Step, StepKind};
use core_runtime::{BrowserClient, DeltaPolicy, LoadProfile, StateUpdate};
use std::time::Instant;

pub fn run_one(url: &str, run_idx: u32) -> RunResult {
    let chrome_path = std::env::var("CHROME_PATH").ok();

    let (raw_html_bytes, raw_html_ttft_ms, raw_success) =
        match measure_raw_dom(url, chrome_path.clone()) {
            Ok((bytes, ms)) => (bytes, ms, true),
            Err(_) => (0, 0, false),
        };

    let (sre_bytes, sre_ttft_ms, sre_success) = match measure_sre(url, chrome_path) {
        Ok((bytes, ms)) => (bytes, ms, true),
        Err(_) => (0, 0, false),
    };

    RunResult {
        run: run_idx,
        raw_html_bytes,
        sre_bytes,
        raw_html_ttft_ms,
        sre_ttft_ms,
        raw_success,
        sre_success,
    }
}

fn measure_raw_dom(url: &str, chrome_path: Option<String>) -> anyhow::Result<(usize, u128)> {
    let client = BrowserClient::new_with_chrome_path(chrome_path)?;
    let page = client.new_page()?;
    let start = Instant::now();
    page.navigate(url)?;
    let html = page.get_content()?;
    let elapsed = start.elapsed().as_millis();
    Ok((html.len(), elapsed))
}

fn measure_sre(url: &str, chrome_path: Option<String>) -> anyhow::Result<(usize, u128)> {
    let client = BrowserClient::new_with_chrome_path(chrome_path)?;
    let page = client.new_page()?;
    let start = Instant::now();
    page.navigate(url)?;
    let state = page.capture_semantic_state(LoadProfile::Minimal)?;
    let elapsed = start.elapsed().as_millis();
    // Measure interactive_elements only — matches what MCP get_state returns to
    // the LLM (ExternalSemanticState). FastSemanticState also contains messages
    // (all non-empty text nodes), which are NOT included in the MCP payload.
    let fast = state.generate_fast_state();
    let json = serde_json::to_string(&fast.interactive_elements)?;
    Ok((json.len(), elapsed))
}

/// Measure cumulative token cost of a multi-step interaction sequence,
/// mirroring the real `mcp-server` `get_state` delta path: the first call is
/// a full state capture, every subsequent call runs the same
/// `select_update`/`DeltaPolicy` decision the production Delta code path uses
/// (`mcp-server/src/lib.rs`), so a delta-to-full fallback mid-sequence shows
/// up as a "full" step kind rather than being silently absorbed into a byte
/// count (see docs/bench-playwright-comparison.md#issue-173).
///
/// `LoadProfile::Minimal` is used throughout (not `Interactive`) so
/// `step_bytes[0]` stays comparable to the existing single-call
/// `measure_sre` numbers already published in the comparison report. This is
/// a deliberate Minimal-only measurement, not an exact reproduction of
/// production `get_state`'s Delta path: the real path captures with
/// `LoadProfile::Interactive` and runs `PromptInjectionSanitizer` before
/// `select_update` (see `mcp-server/src/lib.rs`), neither of which happens
/// here. On pages where the Interactive profile includes extra nodes, or the
/// sanitizer flags content, production byte counts (and even the
/// noop/delta/full decision) could differ from what's reported by this
/// harness (Codex review, PR #192).
///
/// Interactions are fired via `evaluate_script` (a direct DOM `.click()`),
/// bypassing `PageSession::act()`/`PolicyEngine`/audit — this harness only
/// measures the read-side `get_state` payload cost, which is a pure function
/// of captured state and is independent of the action-chain bookkeeping
/// `act()` performs (the same simplification `run_one` already makes).
pub fn run_multi_step(url: &str, step_selectors: &[&str], run_idx: u32) -> MultiStepResult {
    let chrome_path = std::env::var("CHROME_PATH").ok();
    match run_multi_step_inner(url, step_selectors, chrome_path) {
        Ok(steps) => MultiStepResult {
            run: run_idx,
            steps,
            success: true,
        },
        Err(_) => MultiStepResult {
            run: run_idx,
            steps: Vec::new(),
            success: false,
        },
    }
}

fn run_multi_step_inner(
    url: &str,
    step_selectors: &[&str],
    chrome_path: Option<String>,
) -> anyhow::Result<Vec<Step>> {
    let client = BrowserClient::new_with_chrome_path(chrome_path)?;
    let page = client.new_page()?;
    page.navigate(url)?;

    let mut previous = page.capture_semantic_state(LoadProfile::Minimal)?;
    let initial_bytes =
        serde_json::to_string(&previous.generate_fast_state().interactive_elements)?.len();

    let mut steps = Vec::with_capacity(step_selectors.len() + 1);
    steps.push(Step {
        bytes: initial_bytes,
        kind: StepKind::Full,
    });

    for selector in step_selectors {
        let selector_js = serde_json::to_string(selector)?;
        // `headless_chrome::Tab::evaluate` discards CDP's `exceptionDetails`,
        // so a thrown JS error would NOT surface as a Rust `Err` here — it
        // would silently succeed with an `undefined` result. Instead, have
        // the script return whether it found and clicked an element, and
        // fail the run explicitly when it didn't; otherwise a typo'd/stale
        // scenario selector would silently record a phantom "noop" step and
        // inflate the apparent cumulative-cost savings (Codex review, PR #192).
        let found = page.evaluate_script_json(&format!(
            "(() => {{ const el = document.querySelector({selector_js}); \
             if (!el) return false; el.click(); return true; }})();"
        ))?;
        if found != serde_json::Value::Bool(true) {
            anyhow::bail!("step selector matched no element: {selector}");
        }
        let next = page.capture_semantic_state(LoadProfile::Minimal)?;
        let update = next.select_update(Some(&previous), DeltaPolicy::default())?;

        let step = match &update {
            StateUpdate::Noop { .. } => Step {
                bytes: 0,
                kind: StepKind::Noop,
            },
            StateUpdate::Delta { delta } => Step {
                bytes: delta.patch_size_bytes(),
                kind: StepKind::Delta,
            },
            StateUpdate::Full { state } => Step {
                bytes: serde_json::to_string(&state.generate_fast_state().interactive_elements)?
                    .len(),
                kind: StepKind::Full,
            },
        };
        steps.push(step);
        previous = next;
    }

    Ok(steps)
}
