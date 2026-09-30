//! Comprehensive evaluation bench coverage for `bench` (issue #190).
//!
//! Covers the ROI/metrics math and report-writing paths that don't require a
//! live Chrome instance. The browser-backed end-to-end scenarios stay
//! `#[ignore]`d in `tests/bench_integration.rs` and are not duplicated here
//! (see docs/testing.md).

use bench::metrics::{self, AggregatedMetrics, RunResult};
use bench::report;
use serde_json::Value;
use test_bench_support::{EvaluationBench, EvaluationMode};

#[test]
fn test_bench_comprehensive_evaluation_suite() -> anyhow::Result<()> {
    let mut bench = EvaluationBench::new(
        "bench",
        "comprehensive_evaluation",
        EvaluationMode::from_env(),
    );

    bench.run_scenario(
        "roi_cost_savings_accounting",
        "roi_metrics",
        scenario_roi_cost_savings_accounting,
    );
    bench.run_scenario(
        "json_report_matches_aggregated_metrics",
        "report_generation",
        scenario_json_report_matches_aggregated_metrics,
    );

    bench.write_if_configured()?;
    bench.assert_required_scenarios(&[
        "roi_cost_savings_accounting",
        "json_report_matches_aggregated_metrics",
    ])?;
    bench.assert_all_passed()?;
    Ok(())
}

/// End-to-end token/latency aggregation across a mixed batch of successful
/// and failed runs, followed by the cost-savings dollar/percentage math that
/// feeds the printed table and reports.
fn scenario_roi_cost_savings_accounting() -> anyhow::Result<Value> {
    let results = vec![
        RunResult {
            run: 0,
            raw_html_bytes: 8000,
            sre_bytes: 400,
            raw_html_ttft_ms: 100,
            sre_ttft_ms: 50,
            raw_success: true,
            sre_success: true,
        },
        RunResult {
            run: 1,
            raw_html_bytes: 12000,
            sre_bytes: 600,
            raw_html_ttft_ms: 200,
            sre_ttft_ms: 80,
            raw_success: true,
            sre_success: false,
        },
    ];

    let aggregated = metrics::aggregate(&results);
    if aggregated.raw_avg_tokens != 2500 || aggregated.sre_avg_tokens != 100 {
        anyhow::bail!(
            "unexpected token averages: raw={} sre={}",
            aggregated.raw_avg_tokens,
            aggregated.sre_avg_tokens
        );
    }

    let savings = metrics::cost_savings(aggregated.raw_avg_tokens, aggregated.sre_avg_tokens);
    if savings.token_reduction_pct <= 0.0 || savings.gpt4o_savings_usd <= 0.0 {
        anyhow::bail!("expected positive savings when SRE payload is smaller than raw DOM");
    }

    Ok(serde_json::json!({
        "raw_avg_tokens": aggregated.raw_avg_tokens,
        "sre_avg_tokens": aggregated.sre_avg_tokens,
        "token_reduction_pct": savings.token_reduction_pct,
    }))
}

/// The JSON report written for `bench-playwright` consumption must reflect
/// the same aggregated metrics passed into it, so a stale/duplicated
/// computation can't silently diverge from what the table prints.
fn scenario_json_report_matches_aggregated_metrics() -> anyhow::Result<Value> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("report.json");

    let aggregated = AggregatedMetrics {
        raw_avg_tokens: 2000,
        sre_avg_tokens: 100,
        raw_avg_ttft_ms: 150.0,
        sre_avg_ttft_ms: 50.0,
        raw_success_rate: 100.0,
        sre_success_rate: 50.0,
        runs: 2,
    };

    report::write_json(&aggregated, "https://example.com", &path)?;
    let written = std::fs::read_to_string(&path)?;
    let parsed: Value = serde_json::from_str(&written)?;

    let entry = parsed
        .get(0)
        .ok_or_else(|| anyhow::anyhow!("expected a JSON array with one report entry"))?;
    if entry["url"] != "https://example.com" {
        anyhow::bail!("unexpected url in report: {entry}");
    }
    if entry["raw_html"]["avg_tokens"] != 2000 || entry["sre_minimal"]["avg_tokens"] != 100 {
        anyhow::bail!("report tokens do not match aggregated metrics: {entry}");
    }

    Ok(entry.clone())
}
