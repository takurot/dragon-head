use core_runtime::should_skip_browser_tests;
use core_runtime::{sre::LoadProfile, BrowserClient};
use serde_json::json;
use std::time::Instant;

#[path = "support/nfr_metrics.rs"]
mod nfr_metrics;

#[derive(Debug)]
struct CapacityStats {
    trials: usize,
    sessions_target: usize,
    trial_duration_avg_ms: f64,
    trial_duration_p95_ms: f64,
    trial_duration_p99_ms: f64,
    capture_avg_ms: f64,
    capture_p95_ms: f64,
    capture_p99_ms: f64,
    success_rate_min: f64,
}

fn run_capacity_trials(
    profile_name: &str,
    profile: LoadProfile,
    trials: usize,
    sessions_target: usize,
    url: &str,
) -> anyhow::Result<CapacityStats> {
    let mut trial_durations = Vec::with_capacity(trials);
    let mut capture_durations = Vec::with_capacity(trials * sessions_target);
    let mut success_rates = Vec::with_capacity(trials);

    for trial in 0..trials {
        let trial_started = Instant::now();
        let client = BrowserClient::new()?;
        let mut sessions = Vec::with_capacity(sessions_target);

        for _ in 0..sessions_target {
            sessions.push(client.new_page()?);
        }

        for page in &sessions {
            page.navigate(url)?;
        }

        let mut success_count = 0usize;
        for (session_index, page) in sessions.iter().enumerate() {
            let started = Instant::now();
            page.capture_semantic_state(profile).map_err(|error| {
                anyhow::anyhow!(
                    "{profile_name} capacity trial {} session {} capture failed: {error}",
                    trial + 1,
                    session_index + 1
                )
            })?;
            capture_durations.push(started.elapsed());
            success_count += 1;
        }

        let trial_duration = trial_started.elapsed();
        trial_durations.push(trial_duration);
        let success_rate = success_count as f64 / sessions_target as f64;
        success_rates.push(success_rate);
    }

    Ok(CapacityStats {
        trials,
        sessions_target,
        trial_duration_avg_ms: nfr_metrics::average_duration_ms(&trial_durations),
        trial_duration_p95_ms: nfr_metrics::percentile_duration_ms(&trial_durations, 0.95),
        trial_duration_p99_ms: nfr_metrics::percentile_duration_ms(&trial_durations, 0.99),
        capture_avg_ms: nfr_metrics::average_duration_ms(&capture_durations),
        capture_p95_ms: nfr_metrics::percentile_duration_ms(&capture_durations, 0.95),
        capture_p99_ms: nfr_metrics::percentile_duration_ms(&capture_durations, 0.99),
        success_rate_min: nfr_metrics::min_f64(&success_rates),
    })
}

#[test]
fn test_nfr_capacity_targets_by_profile() -> anyhow::Result<()> {
    if should_skip_browser_tests() {
        return Ok(());
    }

    let mode = nfr_metrics::bench_mode();
    let default_trials = if mode == "full" { 4 } else { 2 };
    let default_minimal_sessions = if mode == "full" { 75 } else { 25 };
    let default_visual_sessions = if mode == "full" { 20 } else { 8 };

    let trials = nfr_metrics::env_usize_with_default("NFR_CAPACITY_TRIALS", default_trials);
    let minimal_sessions = nfr_metrics::env_usize_with_default(
        "NFR_CAPACITY_MINIMAL_SESSIONS",
        default_minimal_sessions,
    );
    let visual_sessions = nfr_metrics::env_usize_with_default(
        "NFR_CAPACITY_VISUAL_SESSIONS",
        default_visual_sessions,
    );
    let min_success_rate =
        nfr_metrics::env_u64_with_default("NFR_CAPACITY_MIN_SUCCESS_RATE_PERCENT", 100) as f64
            / 100.0;

    let html = r#"
        <html>
            <body>
                <h1>Session Check</h1>
                <p>Capacity benchmark fixture</p>
                <button id="run">Run</button>
                <img src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO5dM2QAAAAASUVORK5CYII=" alt="tiny" />
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));

    let minimal_stats = run_capacity_trials(
        "minimal",
        LoadProfile::Minimal,
        trials,
        minimal_sessions,
        &url,
    )?;
    let visual_stats =
        run_capacity_trials("visual", LoadProfile::Visual, trials, visual_sessions, &url)?;

    eprintln!(
        "NFR capacity benchmark mode={mode} minimal(target={minimal_sessions},trials={trials},capture_p95={:.3}ms,capture_p99={:.3}ms,success_min={:.2}%) visual(target={visual_sessions},capture_p95={:.3}ms,capture_p99={:.3}ms,success_min={:.2}%)",
        minimal_stats.capture_p95_ms,
        minimal_stats.capture_p99_ms,
        minimal_stats.success_rate_min * 100.0,
        visual_stats.capture_p95_ms,
        visual_stats.capture_p99_ms,
        visual_stats.success_rate_min * 100.0
    );

    nfr_metrics::write_metric(
        "nfr-capacity-minimal",
        json!({
            "metric_id": "nfr-capacity-minimal",
            "mode": mode,
            "values": {
                "trials": minimal_stats.trials,
                "sessions_target": minimal_stats.sessions_target,
                "trial_duration_avg_ms": minimal_stats.trial_duration_avg_ms,
                "trial_duration_p95_ms": minimal_stats.trial_duration_p95_ms,
                "trial_duration_p99_ms": minimal_stats.trial_duration_p99_ms,
                "capture_avg_ms": minimal_stats.capture_avg_ms,
                "capture_p95_ms": minimal_stats.capture_p95_ms,
                "capture_p99_ms": minimal_stats.capture_p99_ms,
                "success_rate_min": minimal_stats.success_rate_min,
            },
            "thresholds": {
                "success_rate_min_min": min_success_rate,
            },
            "display": ["sessions_target", "trials", "capture_p95_ms", "capture_p99_ms", "success_rate_min"],
        }),
    )?;

    nfr_metrics::write_metric(
        "nfr-capacity-visual",
        json!({
            "metric_id": "nfr-capacity-visual",
            "mode": mode,
            "values": {
                "trials": visual_stats.trials,
                "sessions_target": visual_stats.sessions_target,
                "trial_duration_avg_ms": visual_stats.trial_duration_avg_ms,
                "trial_duration_p95_ms": visual_stats.trial_duration_p95_ms,
                "trial_duration_p99_ms": visual_stats.trial_duration_p99_ms,
                "capture_avg_ms": visual_stats.capture_avg_ms,
                "capture_p95_ms": visual_stats.capture_p95_ms,
                "capture_p99_ms": visual_stats.capture_p99_ms,
                "success_rate_min": visual_stats.success_rate_min,
            },
            "thresholds": {
                "success_rate_min_min": min_success_rate,
            },
            "display": ["sessions_target", "trials", "capture_p95_ms", "capture_p99_ms", "success_rate_min"],
        }),
    )?;

    assert!(
        minimal_stats.success_rate_min >= min_success_rate,
        "Minimal profile capacity regression: minimum success_rate {:.2}% below threshold {:.2}%",
        minimal_stats.success_rate_min * 100.0,
        min_success_rate * 100.0
    );
    assert!(
        visual_stats.success_rate_min >= min_success_rate,
        "Visual profile capacity regression: minimum success_rate {:.2}% below threshold {:.2}%",
        visual_stats.success_rate_min * 100.0,
        min_success_rate * 100.0
    );

    Ok(())
}
