use crate::metrics::{cost_savings, AggregatedMetrics};
use anyhow::Result;
use std::path::Path;

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
        format!("-${:.6}", savings.gpt4o_savings_usd)
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
        format!("-${:.6}", savings.claude_savings_usd)
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
| Est. GPT-4o Cost / run | ${raw_gpt:.6} | ${sre_gpt:.6} | -${save_gpt:.6} |
| Est. Claude Cost / run | ${raw_cl:.6} | ${sre_cl:.6} | -${save_cl:.6} |

## Cost Savings Summary

- **Token reduction:** {token_pct:.1}%
- **GPT-4o savings per run:** ${save_gpt:.6} USD
- **Claude savings per run:** ${save_cl:.6} USD

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
        save_gpt = savings.gpt4o_savings_usd,
        raw_cl = m.raw_avg_tokens as f64 * crate::metrics::CLAUDE_COST_PER_TOKEN,
        sre_cl = m.sre_avg_tokens as f64 * crate::metrics::CLAUDE_COST_PER_TOKEN,
        save_cl = savings.claude_savings_usd,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::{aggregate, RunResult};

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
}
