use crate::metrics::{cost_savings, AggregatedMetrics, MultiStepAggregatedMetrics};
use anyhow::Result;
use std::path::Path;

fn format_savings_usd(usd: f64) -> String {
    if usd >= 0.0 {
        format!("-${usd:.6}")
    } else {
        format!("+${:.6}", usd.abs())
    }
}

pub fn print_table(m: &AggregatedMetrics) {
    let savings = cost_savings(m.raw_avg_tokens, m.sre_avg_tokens);
    println!();
    println!(
        "{:<30} {:>15} {:>20} {:>15}",
        "Metric", "Raw DOM", "Dragon Head SRE", "Savings"
    );
    println!("{}", "-".repeat(82));
    println!(
        "{:<30} {:>15} {:>20} {:>14.1}%",
        "Avg Tokens (est.)", m.raw_avg_tokens, m.sre_avg_tokens, savings.token_reduction_pct
    );
    println!(
        "{:<30} {:>14.1}ms {:>19.1}ms {:>15}",
        "Avg TTFT", m.raw_avg_ttft_ms, m.sre_avg_ttft_ms, "-"
    );
    println!(
        "{:<30} {:>14.1}% {:>19.1}% {:>15}",
        "Success Rate", m.raw_success_rate, m.sre_success_rate, "-"
    );
    println!(
        "{:<30} {:>14} {:>20} {:>15}",
        "Est. GPT-4o Cost / run",
        format!(
            "${:.6}",
            m.raw_avg_tokens as f64 * crate::metrics::GPT4O_COST_PER_TOKEN
        ),
        format!(
            "${:.6}",
            m.sre_avg_tokens as f64 * crate::metrics::GPT4O_COST_PER_TOKEN
        ),
        format_savings_usd(savings.gpt4o_savings_usd)
    );
    println!(
        "{:<30} {:>14} {:>20} {:>15}",
        "Est. Claude Cost / run",
        format!(
            "${:.6}",
            m.raw_avg_tokens as f64 * crate::metrics::CLAUDE_COST_PER_TOKEN
        ),
        format!(
            "${:.6}",
            m.sre_avg_tokens as f64 * crate::metrics::CLAUDE_COST_PER_TOKEN
        ),
        format_savings_usd(savings.claude_savings_usd)
    );
    println!("{}", "-".repeat(82));
    println!(
        "Token reduction: {:.1}%  |  Runs: {}",
        savings.token_reduction_pct, m.runs
    );
    println!();
}

pub fn write_markdown(
    m: &AggregatedMetrics,
    url: &str,
    task: Option<&str>,
    path: &Path,
) -> Result<()> {
    let savings = cost_savings(m.raw_avg_tokens, m.sre_avg_tokens);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let content = build_markdown(m, url, task, &savings, now);
    std::fs::write(path, content)?;
    Ok(())
}

/// Write a JSON report consumable by the `bench-playwright` compare script.
///
/// Schema mirrors `DragonHeadMetrics` in `bench-playwright/src/compare.ts`.
pub fn write_json(m: &AggregatedMetrics, url: &str, path: &Path) -> Result<()> {
    let s = cost_savings(m.raw_avg_tokens, m.sre_avg_tokens);
    let json = serde_json::json!([{
        "url": url,
        "runs": m.runs,
        "raw_html": {
            "avg_tokens": m.raw_avg_tokens,
            "avg_ttft_ms": m.raw_avg_ttft_ms,
            "success_rate": m.raw_success_rate
        },
        "sre_minimal": {
            "avg_tokens": m.sre_avg_tokens,
            "avg_ttft_ms": m.sre_avg_ttft_ms,
            "success_rate": m.sre_success_rate
        },
        "cost_savings": {
            "token_reduction_pct": s.token_reduction_pct,
            "gpt4o_savings_usd": s.gpt4o_savings_usd,
            "claude_savings_usd": s.claude_savings_usd
        }
    }]);
    std::fs::write(path, serde_json::to_string_pretty(&json)?)?;
    Ok(())
}

fn build_markdown(
    m: &AggregatedMetrics,
    url: &str,
    task: Option<&str>,
    savings: &crate::metrics::CostSavings,
    timestamp_secs: u64,
) -> String {
    let task_line = task
        .map(|t| format!("**Task:** {t}\n\n"))
        .unwrap_or_default();

    format!(
        r#"# Dragon Head ROI Comparison Report

**URL:** {url}
{task_line}**Runs:** {runs}
**Timestamp (unix):** {timestamp_secs}

## Results

| Metric | Raw DOM | Dragon Head SRE | Savings |
|--------|--------:|----------------:|--------:|
| Avg Tokens (est.) | {raw_tokens} | {sre_tokens} | {token_pct:.1}% |
| Avg TTFT (ms) | {raw_ttft:.1} | {sre_ttft:.1} | — |
| Success Rate | {raw_sr:.1}% | {sre_sr:.1}% | — |
| Est. GPT-4o Cost / run | ${raw_gpt:.6} | ${sre_gpt:.6} | {save_gpt_fmt} |
| Est. Claude Cost / run | ${raw_cl:.6} | ${sre_cl:.6} | {save_cl_fmt} |

## Cost Savings Summary

- **Token reduction:** {token_pct:.1}%
- **GPT-4o savings per run:** {save_gpt_fmt} USD
- **Claude savings per run:** {save_cl_fmt} USD

> Token estimates use the standard approximation of 1 token ≈ 4 characters.
> Pricing: GPT-4o input $5/1M tokens, Claude claude-sonnet-4-6 input $3/1M tokens.
"#,
        url = url,
        task_line = task_line,
        runs = m.runs,
        timestamp_secs = timestamp_secs,
        raw_tokens = m.raw_avg_tokens,
        sre_tokens = m.sre_avg_tokens,
        token_pct = savings.token_reduction_pct,
        raw_ttft = m.raw_avg_ttft_ms,
        sre_ttft = m.sre_avg_ttft_ms,
        raw_sr = m.raw_success_rate,
        sre_sr = m.sre_success_rate,
        raw_gpt = m.raw_avg_tokens as f64 * crate::metrics::GPT4O_COST_PER_TOKEN,
        sre_gpt = m.sre_avg_tokens as f64 * crate::metrics::GPT4O_COST_PER_TOKEN,
        save_gpt_fmt = format_savings_usd(savings.gpt4o_savings_usd),
        raw_cl = m.raw_avg_tokens as f64 * crate::metrics::CLAUDE_COST_PER_TOKEN,
        sre_cl = m.sre_avg_tokens as f64 * crate::metrics::CLAUDE_COST_PER_TOKEN,
        save_cl_fmt = format_savings_usd(savings.claude_savings_usd),
    )
}

/// Print the cumulative multi-step comparison to stdout.
///
/// `sample_kinds` is the per-step `StateUpdate` kind ("full" | "delta" |
/// "noop") from a representative successful run — surfaced so a delta
/// fallback to full mid-sequence is visible, not hidden inside the byte
/// count (see docs/bench-playwright-comparison.md#issue-173).
pub fn print_multi_step_table(m: &MultiStepAggregatedMetrics, sample_kinds: &[&str]) {
    println!();
    println!(
        "{:<10} {:<8} {:>15} {:>20}",
        "Step", "Kind", "Avg Bytes", "Cumulative Bytes"
    );
    println!("{}", "-".repeat(56));
    for i in 0..m.steps {
        let kind = sample_kinds.get(i).copied().unwrap_or("?");
        println!(
            "{:<10} {:<8} {:>15.1} {:>20.1}",
            i, kind, m.avg_step_bytes[i], m.cumulative_avg_bytes[i]
        );
    }
    println!("{}", "-".repeat(56));
    println!("Success rate: {:.1}%  |  Runs: {}", m.success_rate, m.runs);
    println!();
}

pub fn write_multi_step_markdown(
    m: &MultiStepAggregatedMetrics,
    sample_kinds: &[&str],
    url: &str,
    task: Option<&str>,
    path: &Path,
) -> Result<()> {
    let content = build_multi_step_markdown(m, sample_kinds, url, task);
    std::fs::write(path, content)?;
    Ok(())
}

/// Write a JSON report of the cumulative multi-step comparison, consumable
/// by the `bench-playwright` compare script (schema mirrors
/// `MultiStepDragonHeadMetrics` in `bench-playwright/src/metrics.ts`).
pub fn write_multi_step_json(
    m: &MultiStepAggregatedMetrics,
    sample_kinds: &[&str],
    url: &str,
    path: &Path,
) -> Result<()> {
    let json = serde_json::json!({
        "url": url,
        "runs": m.runs,
        "steps": m.steps,
        "success_rate": m.success_rate,
        "avg_step_bytes": m.avg_step_bytes,
        "cumulative_avg_bytes": m.cumulative_avg_bytes,
        "sample_step_kinds": sample_kinds,
    });
    std::fs::write(path, serde_json::to_string_pretty(&json)?)?;
    Ok(())
}

fn build_multi_step_markdown(
    m: &MultiStepAggregatedMetrics,
    sample_kinds: &[&str],
    url: &str,
    task: Option<&str>,
) -> String {
    let task_line = task
        .map(|t| format!("**Task:** {t}\n\n"))
        .unwrap_or_default();

    let mut rows = String::new();
    for i in 0..m.steps {
        let kind = sample_kinds.get(i).copied().unwrap_or("?");
        rows.push_str(&format!(
            "| {i} | {kind} | {:.1} | {:.1} |\n",
            m.avg_step_bytes[i], m.cumulative_avg_bytes[i]
        ));
    }

    format!(
        r#"# Dragon Head Multi-Step Delta Cost Report

**URL:** {url}
{task_line}**Runs:** {runs}
**Steps:** {steps}
**Success rate:** {success_rate:.1}%

## Per-Step Cost

| Step | Kind | Avg Bytes | Cumulative Bytes |
|-----:|------|----------:|-----------------:|
{rows}
> Step 0 is the initial full-state capture. Steps 1..N are re-captures after
> an interaction, each measured via the same `select_update`/`DeltaPolicy`
> decision `mcp-server`'s `get_state` Delta path uses in production — "delta"
> means an RFC 6902 patch was sent, "full" means the delta policy fell back
> to a full re-send, "noop" means the state hash was unchanged.
"#,
        url = url,
        task_line = task_line,
        runs = m.runs,
        steps = m.steps,
        success_rate = m.success_rate,
        rows = rows,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{
        aggregate, aggregate_multi_step, MultiStepResult, RunResult, Step, StepKind,
    };

    fn sample_metrics() -> AggregatedMetrics {
        let results = vec![RunResult {
            run: 0,
            raw_html_bytes: 40_000,
            sre_bytes: 2_000,
            raw_html_ttft_ms: 350,
            sre_ttft_ms: 90,
            raw_success: true,
            sre_success: true,
        }];
        aggregate(&results)
    }

    #[test]
    fn markdown_contains_required_headers() {
        let m = sample_metrics();
        let savings = cost_savings(m.raw_avg_tokens, m.sre_avg_tokens);
        let md = build_markdown(
            &m,
            "https://example.com",
            Some("Load homepage"),
            &savings,
            0,
        );
        assert!(md.contains("# Dragon Head ROI Comparison Report"));
        assert!(md.contains("| Metric | Raw DOM | Dragon Head SRE | Savings |"));
        assert!(md.contains("**URL:** https://example.com"));
        assert!(md.contains("**Task:** Load homepage"));
        assert!(md.contains("Cost Savings Summary"));
        assert!(md.contains("Token reduction:"));
    }

    #[test]
    fn markdown_without_task_omits_task_line() {
        let m = sample_metrics();
        let savings = cost_savings(m.raw_avg_tokens, m.sre_avg_tokens);
        let md = build_markdown(&m, "https://example.com", None, &savings, 0);
        assert!(!md.contains("**Task:**"));
    }

    #[test]
    fn markdown_contains_correct_token_counts() {
        let m = sample_metrics();
        // raw: 40000/4 = 10000, sre: 2000/4 = 500
        let savings = cost_savings(m.raw_avg_tokens, m.sre_avg_tokens);
        let md = build_markdown(&m, "https://example.com", None, &savings, 0);
        assert!(md.contains("10000"));
        assert!(md.contains("500"));
    }

    #[test]
    fn write_markdown_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.md");
        let m = sample_metrics();
        write_markdown(&m, "https://example.com", None, &path).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Dragon Head ROI"));
    }

    #[test]
    fn write_json_produces_valid_dragon_head_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.json");
        let m = sample_metrics();
        write_json(&m, "https://example.com", &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        // Output is a JSON array with one object per URL
        let entry = &v[0];
        assert_eq!(entry["url"], "https://example.com");
        assert_eq!(entry["runs"], 1_u64);
        assert_eq!(entry["raw_html"]["avg_tokens"], 10_000_u64); // 40000/4
        assert_eq!(entry["sre_minimal"]["avg_tokens"], 500_u64); // 2000/4
        let reduction = entry["cost_savings"]["token_reduction_pct"]
            .as_f64()
            .unwrap();
        assert!(
            (reduction - 95.0).abs() < 0.1,
            "expected ~95% reduction, got {reduction}"
        );
    }

    fn sample_multi_step_metrics() -> (MultiStepAggregatedMetrics, Vec<&'static str>) {
        let results = vec![MultiStepResult {
            run: 0,
            steps: vec![
                Step {
                    bytes: 4000,
                    kind: StepKind::Full,
                },
                Step {
                    bytes: 40,
                    kind: StepKind::Delta,
                },
                Step {
                    bytes: 30,
                    kind: StepKind::Delta,
                },
            ],
            success: true,
        }];
        (
            aggregate_multi_step(&results),
            vec!["full", "delta", "delta"],
        )
    }

    #[test]
    fn multi_step_markdown_contains_required_headers() {
        let (m, kinds) = sample_multi_step_metrics();
        let md = build_multi_step_markdown(&m, &kinds, "https://example.com", Some("Filter cycle"));
        assert!(md.contains("# Dragon Head Multi-Step Delta Cost Report"));
        assert!(md.contains("| Step | Kind | Avg Bytes | Cumulative Bytes |"));
        assert!(md.contains("**Task:** Filter cycle"));
    }

    #[test]
    fn multi_step_markdown_rows_show_kind_and_cumulative_bytes() {
        let (m, kinds) = sample_multi_step_metrics();
        let md = build_multi_step_markdown(&m, &kinds, "https://example.com", None);
        // Step 0 is the full capture; step 1/2 are deltas whose cumulative
        // total must include the initial full-state cost.
        assert!(md.contains("| 0 | full | 4000.0 | 4000.0 |"));
        assert!(md.contains("| 1 | delta | 40.0 | 4040.0 |"));
        assert!(md.contains("| 2 | delta | 30.0 | 4070.0 |"));
    }

    #[test]
    fn write_multi_step_json_produces_expected_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multi-step.json");
        let (m, kinds) = sample_multi_step_metrics();
        write_multi_step_json(&m, &kinds, "https://example.com", &path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["url"], "https://example.com");
        assert_eq!(v["steps"], 3_u64);
        assert_eq!(v["cumulative_avg_bytes"][2].as_f64().unwrap(), 4070.0);
        assert_eq!(v["sample_step_kinds"][1], "delta");
    }
}
