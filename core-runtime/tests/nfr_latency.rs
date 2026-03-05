use core_runtime::{sre::LoadProfile, BrowserClient};
use serde_json::json;
use std::time::Instant;

#[path = "support/nfr_metrics.rs"]
mod nfr_metrics;

fn should_skip() -> bool {
    std::env::var("CI").is_ok() && std::env::var("CHROME_INSTALLED").is_err()
}

#[test]
fn test_nfr_state_update_latency_under_100ms() -> anyhow::Result<()> {
    if should_skip() {
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

    let html = r#"
        <html>
            <body>
                <ul id="list">
                    <li>Item 1</li>
                </ul>
                <script>
                    window.addItems = (count) => {
                        const list = document.getElementById('list');
                        list.innerHTML = '';
                        for (let i = 0; i < count; i++) {
                            const li = document.createElement('li');
                            li.innerText = 'New Item ' + i;
                            list.appendChild(li);
                        }
                    };
                </script>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    // Initial capture (Full State)
    let _initial_state = page.capture_semantic_state(LoadProfile::Minimal)?;

    let mut samples = Vec::with_capacity(trials);
    for idx in 0..trials {
        // Keep mutation under the NFR precondition (< 50 changed nodes).
        let node_count = 45 - (idx % 5);
        page.evaluate_script(&format!("window.addItems({node_count})"))?;

        let start = Instant::now();
        let _delta_state = page.capture_semantic_state(LoadProfile::Minimal)?;
        samples.push(start.elapsed());
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
