use core_runtime::{sre::LoadProfile, BrowserClient};
use serde_json::json;
use core_runtime::should_skip_browser_tests;

#[path = "support/nfr_metrics.rs"]
mod nfr_metrics;

#[test]
fn test_nfr_bandwidth_95_percent_reduction() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let mode = nfr_metrics::bench_mode();
    let default_trials = if mode == "full" { 10 } else { 5 };
    let trials = nfr_metrics::env_usize_with_default("NFR_BANDWIDTH_TRIALS", default_trials);
    let reduction_min_pct =
        nfr_metrics::env_u64_with_default("NFR_BANDWIDTH_REDUCTION_MIN_PERCENT", 95) as f64;

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let mut reductions_pct = Vec::with_capacity(trials);
    let mut minimal_bytes = Vec::with_capacity(trials);
    let mut standard_bytes = Vec::with_capacity(trials);

    for trial in 0..trials {
        let removable_nodes = (0..(250 + (trial * 15)))
            .map(|i| {
                format!(
                    "<script>console.log('trial{trial}-{i}')</script><style>.x{trial}_{i}{{color:red;}}</style><img src=\"t{trial}_{i}.png\" alt=\"tracker\" />"
                )
            })
            .collect::<String>();
        let html = format!(
            r#"
            <html>
                <head>
                    <style>
                        body {{ font-family: sans-serif; }}
                        .content {{ padding: 20px; }}
                        .ad {{ width: 300px; height: 250px; background: red; }}
                    </style>
                </head>
                <body>
                    <div class="content">
                        <h1>Main Article Trial {trial}</h1>
                        <p>This is the main content.</p>
                        {removable_nodes}
                        <div class="ad">Advertisement</div>
                        <button id="btn">Click me</button>
                    </div>
                </body>
            </html>
        "#
        );
        let url = format!("data:text/html,{}", urlencoding::encode(&html));
        page.navigate(&url)?;

        let minimal_state = page.capture_semantic_state(LoadProfile::Minimal)?;
        let minimal_json = serde_json::to_string(&minimal_state)?;

        let standard_state = page.capture_semantic_state(LoadProfile::Interactive)?;
        let standard_json = serde_json::to_string(&standard_state)?;

        let min_size = minimal_json.len() as f64;
        let std_size = standard_json.len() as f64;
        assert!(std_size > 0.0, "standard profile payload must be non-zero");

        let reduction_pct = (1.0 - (min_size / std_size)) * 100.0;
        minimal_bytes.push(min_size);
        standard_bytes.push(std_size);
        reductions_pct.push(reduction_pct);
    }

    let reduction_avg_pct = nfr_metrics::average_f64(&reductions_pct);
    let reduction_p95_pct = nfr_metrics::percentile_f64(&reductions_pct, 0.95);
    let reduction_p99_pct = nfr_metrics::percentile_f64(&reductions_pct, 0.99);
    let reduction_min_observed_pct = nfr_metrics::min_f64(&reductions_pct);
    let minimal_avg_bytes = nfr_metrics::average_f64(&minimal_bytes);
    let standard_avg_bytes = nfr_metrics::average_f64(&standard_bytes);

    eprintln!(
        "NFR bandwidth benchmark mode={mode} trials={trials} reduction(avg={reduction_avg_pct:.2}%,p95={reduction_p95_pct:.2}%,p99={reduction_p99_pct:.2}%,min={reduction_min_observed_pct:.2}%) threshold(min>={reduction_min_pct:.2}%)"
    );

    nfr_metrics::write_metric(
        "nfr-bandwidth",
        json!({
            "metric_id": "nfr-bandwidth",
            "mode": mode,
            "values": {
                "trials": trials,
                "minimal_avg_bytes": minimal_avg_bytes,
                "standard_avg_bytes": standard_avg_bytes,
                "reduction_avg_pct": reduction_avg_pct,
                "reduction_p95_pct": reduction_p95_pct,
                "reduction_p99_pct": reduction_p99_pct,
                "reduction_min_pct": reduction_min_observed_pct,
            },
            "thresholds": {
                "reduction_min_pct_min": reduction_min_pct,
            },
            "display": ["trials", "reduction_avg_pct", "reduction_p95_pct", "reduction_p99_pct", "reduction_min_pct"],
        }),
    )?;

    assert!(
        reduction_min_observed_pct >= reduction_min_pct,
        "Bandwidth reduction regression: expected minimum reduction >= {:.2}%, got {:.2}%",
        reduction_min_pct,
        reduction_min_observed_pct
    );

    Ok(())
}
