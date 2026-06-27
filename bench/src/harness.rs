use crate::metrics::RunResult;
use core_runtime::{BrowserClient, LoadProfile};
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
