use crate::metrics::{MultiStepResult, RunResult};
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
/// `measure_sre` numbers already published in the comparison report.
///
/// Interactions are fired via `evaluate_script` (a direct DOM `.click()`),
/// bypassing `PageSession::act()`/`PolicyEngine`/audit — this harness only
/// measures the read-side `get_state` payload cost, which is a pure function
/// of captured state and is independent of the action-chain bookkeeping
/// `act()` performs (the same simplification `run_one` already makes).
pub fn run_multi_step(url: &str, step_selectors: &[&str], run_idx: u32) -> MultiStepResult {
    let chrome_path = std::env::var("CHROME_PATH").ok();
    match run_multi_step_inner(url, step_selectors, chrome_path) {
        Ok((step_bytes, step_kinds)) => MultiStepResult {
            run: run_idx,
            step_bytes,
            step_kinds,
            success: true,
        },
        Err(_) => MultiStepResult {
            run: run_idx,
            step_bytes: Vec::new(),
            step_kinds: Vec::new(),
            success: false,
        },
    }
}

fn run_multi_step_inner(
    url: &str,
    step_selectors: &[&str],
    chrome_path: Option<String>,
) -> anyhow::Result<(Vec<usize>, Vec<&'static str>)> {
    let client = BrowserClient::new_with_chrome_path(chrome_path)?;
    let page = client.new_page()?;
    page.navigate(url)?;

    let mut previous = page.capture_semantic_state(LoadProfile::Minimal)?;
    let initial_bytes =
        serde_json::to_string(&previous.generate_fast_state().interactive_elements)?.len();

    let mut step_bytes = Vec::with_capacity(step_selectors.len() + 1);
    let mut step_kinds = Vec::with_capacity(step_selectors.len() + 1);
    step_bytes.push(initial_bytes);
    step_kinds.push("full");

    for selector in step_selectors {
        let selector_js = serde_json::to_string(selector)?;
        page.evaluate_script(&format!("document.querySelector({selector_js})?.click();"))?;
        let next = page.capture_semantic_state(LoadProfile::Minimal)?;
        let update = next.select_update(Some(&previous), DeltaPolicy::default())?;

        let (bytes, kind) = match &update {
            StateUpdate::Noop { .. } => (0, "noop"),
            StateUpdate::Delta { delta } => (delta.patch_size_bytes(), "delta"),
            StateUpdate::Full { state } => (
                serde_json::to_string(&state.generate_fast_state().interactive_elements)?.len(),
                "full",
            ),
        };
        step_bytes.push(bytes);
        step_kinds.push(kind);
        previous = next;
    }

    Ok((step_bytes, step_kinds))
}
