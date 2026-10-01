//! Comprehensive evaluation bench coverage for `bench` (issue #190).
//!
//! Covers the ROI/metrics math and report-writing paths that don't require a
//! live Chrome instance. The browser-backed end-to-end scenarios stay
//! `#[ignore]`d in `tests/bench_integration.rs` and are not duplicated here
//! (see docs/testing.md).

use bench::metrics::{self, AggregatedMetrics, GovernanceOutcome, GovernanceRunResult, RunResult};
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
    bench.run_scenario(
        "governance_failures_and_unknowns_remain_visible",
        "governance_metrics",
        scenario_governance_failures_and_unknowns_remain_visible,
    );

    bench.write_if_configured()?;
    bench.assert_required_scenarios(&[
        "roi_cost_savings_accounting",
        "json_report_matches_aggregated_metrics",
        "governance_failures_and_unknowns_remain_visible",
    ])?;
    bench.assert_all_passed()?;
    Ok(())
}

fn scenario_governance_failures_and_unknowns_remain_visible() -> anyhow::Result<Value> {
    let runs = vec![
        GovernanceRunResult {
            run: 0,
            raw: GovernanceOutcome {
                observable_actions: 3,
                wrong_actions: 1,
                mutation_observed: true,
                safety_probe_attempted: true,
                safety_violation: Some(true),
                ..Default::default()
            },
            sre: GovernanceOutcome {
                completed: true,
                observable_actions: 4,
                mutation_observed: true,
                safety_probe_attempted: true,
                safety_violation: Some(false),
                human_interventions: 1,
                audit_complete: Some(false),
                audit_gaps: vec!["missing get_state".to_owned()],
                ..Default::default()
            },
        },
        GovernanceRunResult {
            run: 1,
            raw: GovernanceOutcome::default(),
            sre: GovernanceOutcome {
                safety_probe_attempted: true,
                audit_complete: Some(false),
                errors: vec!["startup failed".to_owned()],
                ..Default::default()
            },
        },
    ];
    let report = report::GovernanceReport::new(runs);
    let metrics = &report.metrics;
    anyhow::ensure!(
        metrics.sre_completion.numerator == 1 && metrics.sre_completion.denominator == 2
    );
    anyhow::ensure!(metrics.sre_selector_recovery.rate_pct == Some(100.0));
    anyhow::ensure!(
        metrics.raw_wrong_actions.numerator == 1 && metrics.raw_wrong_actions.denominator == 3
    );
    anyhow::ensure!(
        metrics.sre_safety_violations.denominator == 1 && metrics.sre_unobserved_safety_probes == 1
    );
    anyhow::ensure!(metrics.sre_avg_human_interventions == Some(0.5));
    anyhow::ensure!(metrics.raw_audit_completeness.is_none());
    anyhow::ensure!(
        metrics.sre_audit_completeness.numerator == 0
            && metrics.sre_audit_completeness.denominator == 2
    );
    let json = serde_json::to_value(report)?;
    anyhow::ensure!(json["runs"].as_array().is_some_and(|runs| runs.len() == 2));
    anyhow::ensure!(json["runs"][0]["sre"]["audit_gaps"][0] == "missing get_state");
    anyhow::ensure!(json["runs"][1]["sre"]["errors"][0] == "startup failed");
    Ok(json["metrics"].clone())
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
