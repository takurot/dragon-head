use core_runtime::{
    sre::{
        normalize_dom, normalize_dom_with_refinement, LoadProfile, SemanticNode, SemanticState,
        SubtreeRefinementConfig,
    },
    BrowserClient,
};
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    time::Instant,
};

#[path = "support/nfr_metrics.rs"]
mod nfr_metrics;

#[test]
fn test_nfr_state_update_latency_under_100ms() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let mode = nfr_metrics::bench_mode();
    let default_trials = if mode == "full" { 60 } else { 25 };
    let trials = nfr_metrics::env_usize_with_default("NFR_LATENCY_TRIALS", default_trials);
    let p95_limit_ms = nfr_metrics::env_u64_with_default(
        "NFR_LATENCY_P95_LIMIT_MS",
        if mode == "full" { 100 } else { 260 },
    );
    let p99_limit_ms = nfr_metrics::env_u64_with_default(
        "NFR_LATENCY_P99_LIMIT_MS",
        if mode == "full" { 130 } else { 340 },
    );

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let initial_items = (0..45)
        .map(|idx| format!("<li>Item {idx}</li>"))
        .collect::<Vec<_>>()
        .join("");
    let html = format!(
        r#"
        <html>
            <body>
                <ul id="list">{initial_items}</ul>
                <script>
                    window.updateItems = (count, cycle) => {{
                        const items = document.querySelectorAll('#list li');
                        for (let i = 0; i < items.length; i++) {{
                            items[i].innerText = i < count
                                ? `Updated ${{cycle}}-${{i}}`
                                : `Item ${{i}}`;
                        }}
                    }};
                </script>
            </body>
        </html>
    "#
    );
    let url = format!("data:text/html,{}", urlencoding::encode(&html));
    page.navigate(&url)?;

    // Warm the DOM snapshot once, then measure only the SRE update path.
    let initial_root = page.get_document_node()?;
    let initial_semantic_root = normalize_dom(LoadProfile::Minimal, &initial_root)?;
    let mut previous_state = SemanticState::new(initial_semantic_root, LoadProfile::Minimal);
    let mut cached_paths = build_semantic_path_index(previous_state.root());

    let mut samples = Vec::with_capacity(trials);
    for idx in 0..trials {
        // Keep mutation under the NFR precondition (< 50 changed nodes).
        let node_count = 10 + (idx % 5);
        page.evaluate_script(&format!("window.updateItems({node_count}, {idx})"))?;
        let root = page.get_document_node()?;
        let dirty_paths = HashSet::from(["root/#document/html/body/ul".to_string()]);

        let start = Instant::now();
        let semantic_root = normalize_dom_with_refinement(
            LoadProfile::Minimal,
            &root,
            SubtreeRefinementConfig {
                dirty_paths: &dirty_paths,
                cached_paths: &cached_paths,
                cached_root: previous_state.root(),
            },
        )?;
        let next_state = SemanticState::new(semantic_root, LoadProfile::Minimal);
        samples.push(start.elapsed());
        cached_paths = build_semantic_path_index(next_state.root());
        previous_state = next_state;
    }

    let avg_ms = nfr_metrics::average_duration_ms(&samples);
    let p95_ms = nfr_metrics::percentile_duration_ms(&samples, 0.95);
    let p99_ms = nfr_metrics::percentile_duration_ms(&samples, 0.99);
    let max_ms = nfr_metrics::max_duration_ms(&samples);

    eprintln!(
        "NFR latency benchmark mode={mode} trials={trials} avg={avg_ms:.3}ms p95={p95_ms:.3}ms p99={p99_ms:.3}ms max={max_ms:.3}ms limits(p95<={p95_limit_ms}ms,p99<={p99_limit_ms}ms)"
    );

    nfr_metrics::write_metric(
        "nfr-latency",
        json!({
            "metric_id": "nfr-latency",
            "mode": mode,
            "values": {
                "trials": trials,
                "avg_ms": avg_ms,
                "p95_ms": p95_ms,
                "p99_ms": p99_ms,
                "max_ms": max_ms,
            },
            "thresholds": {
                "p95_ms_max": p95_limit_ms as f64,
                "p99_ms_max": p99_limit_ms as f64,
            },
            "display": ["trials", "avg_ms", "p95_ms", "p99_ms", "max_ms"],
        }),
    )?;

    assert!(
        p95_ms <= p95_limit_ms as f64,
        "State Update Latency p95 regression: expected <= {}ms, got {:.3}ms",
        p95_limit_ms,
        p95_ms
    );
    assert!(
        p99_ms <= p99_limit_ms as f64,
        "State Update Latency p99 regression: expected <= {}ms, got {:.3}ms",
        p99_limit_ms,
        p99_ms
    );

    Ok(())
}

fn build_semantic_path_index(root: &SemanticNode) -> HashMap<String, Vec<usize>> {
    let mut counts = HashMap::new();
    count_semantic_paths(root, "root", &mut counts);

    let mut index = HashMap::new();
    let mut current_child_path = Vec::new();
    collect_unique_semantic_paths(root, "root", &counts, &mut current_child_path, &mut index);
    index
}

fn count_semantic_paths(
    node: &SemanticNode,
    parent_path: &str,
    counts: &mut HashMap<String, usize>,
) {
    let current_path = semantic_path(parent_path, &node.role);
    *counts.entry(current_path.clone()).or_insert(0) += 1;

    for child in &node.children {
        count_semantic_paths(child, &current_path, counts);
    }
}

fn collect_unique_semantic_paths(
    node: &SemanticNode,
    parent_path: &str,
    counts: &HashMap<String, usize>,
    current_child_path: &mut Vec<usize>,
    index: &mut HashMap<String, Vec<usize>>,
) {
    let current_path = semantic_path(parent_path, &node.role);
    if counts.get(&current_path).copied() == Some(1) {
        index.insert(current_path.clone(), current_child_path.clone());
    }

    for (child_index, child) in node.children.iter().enumerate() {
        current_child_path.push(child_index);
        collect_unique_semantic_paths(child, &current_path, counts, current_child_path, index);
        current_child_path.pop();
    }
}

fn semantic_path(parent_path: &str, role: &str) -> String {
    format!("{}/{}", parent_path, role.to_lowercase())
}
