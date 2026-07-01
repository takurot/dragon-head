pub const GPT4O_COST_PER_TOKEN: f64 = 5.0 / 1_000_000.0;
pub const CLAUDE_COST_PER_TOKEN: f64 = 3.0 / 1_000_000.0;

#[derive(Debug, Clone)]
pub struct RunResult {
    pub run: u32,
    pub raw_html_bytes: usize,
    pub sre_bytes: usize,
    pub raw_html_ttft_ms: u128,
    pub sre_ttft_ms: u128,
    pub raw_success: bool,
    pub sre_success: bool,
}

#[derive(Debug, Clone)]
pub struct AggregatedMetrics {
    pub raw_avg_tokens: u64,
    pub sre_avg_tokens: u64,
    pub raw_avg_ttft_ms: f64,
    pub sre_avg_ttft_ms: f64,
    pub raw_success_rate: f64,
    pub sre_success_rate: f64,
    pub runs: usize,
}

/// Positive values = savings; negative values = cost increase (SRE > raw DOM).
#[derive(Debug, Clone)]
pub struct CostSavings {
    pub token_reduction_pct: f64,
    pub gpt4o_savings_usd: f64,
    pub claude_savings_usd: f64,
}

pub fn estimate_tokens(byte_count: usize) -> u64 {
    (byte_count / 4) as u64
}

pub fn aggregate(results: &[RunResult]) -> AggregatedMetrics {
    let n = results.len();
    if n == 0 {
        return AggregatedMetrics {
            raw_avg_tokens: 0,
            sre_avg_tokens: 0,
            raw_avg_ttft_ms: 0.0,
            sre_avg_ttft_ms: 0.0,
            raw_success_rate: 0.0,
            sre_success_rate: 0.0,
            runs: 0,
        };
    }

    // Only include successful runs in token/latency averages to avoid failed-run zeros
    // skewing the ROI numbers. Success rate still counts all runs.
    let raw_ok: Vec<&RunResult> = results.iter().filter(|r| r.raw_success).collect();
    let sre_ok: Vec<&RunResult> = results.iter().filter(|r| r.sre_success).collect();

    let raw_avg_tokens = if raw_ok.is_empty() {
        0
    } else {
        raw_ok
            .iter()
            .map(|r| estimate_tokens(r.raw_html_bytes))
            .sum::<u64>()
            / raw_ok.len() as u64
    };
    let sre_avg_tokens = if sre_ok.is_empty() {
        0
    } else {
        sre_ok
            .iter()
            .map(|r| estimate_tokens(r.sre_bytes))
            .sum::<u64>()
            / sre_ok.len() as u64
    };
    let raw_avg_ttft_ms = if raw_ok.is_empty() {
        0.0
    } else {
        raw_ok.iter().map(|r| r.raw_html_ttft_ms).sum::<u128>() as f64 / raw_ok.len() as f64
    };
    let sre_avg_ttft_ms = if sre_ok.is_empty() {
        0.0
    } else {
        sre_ok.iter().map(|r| r.sre_ttft_ms).sum::<u128>() as f64 / sre_ok.len() as f64
    };

    AggregatedMetrics {
        raw_avg_tokens,
        sre_avg_tokens,
        raw_avg_ttft_ms,
        sre_avg_ttft_ms,
        raw_success_rate: raw_ok.len() as f64 / n as f64 * 100.0,
        sre_success_rate: sre_ok.len() as f64 / n as f64 * 100.0,
        runs: n,
    }
}

pub fn cost_savings(raw_tokens: u64, sre_tokens: u64) -> CostSavings {
    let token_reduction_pct = if raw_tokens == 0 {
        0.0
    } else {
        (1.0 - sre_tokens as f64 / raw_tokens as f64) * 100.0
    };
    // Use signed delta so a cost increase (SRE > raw) shows as negative savings.
    let token_delta = raw_tokens as i64 - sre_tokens as i64;
    CostSavings {
        token_reduction_pct,
        gpt4o_savings_usd: token_delta as f64 * GPT4O_COST_PER_TOKEN,
        claude_savings_usd: token_delta as f64 * CLAUDE_COST_PER_TOKEN,
    }
}

/// One run of a multi-step interaction sequence.
///
/// `step_bytes[0]` is the initial full-state capture; `step_bytes[1..]` are
/// the per-step payload sent after each interaction (an RFC 6902 patch, a
/// full re-send on `DeltaPolicy` fallback, or 0 for a no-op).
/// `step_kinds` is parallel to `step_bytes` and records which `StateUpdate`
/// variant produced each entry ("full" | "delta" | "noop"), so a delta
/// fallback to full mid-sequence is visible rather than hidden inside a byte
/// count (see docs/bench-playwright-comparison.md caveats).
#[derive(Debug, Clone)]
pub struct MultiStepResult {
    pub run: u32,
    pub step_bytes: Vec<usize>,
    pub step_kinds: Vec<&'static str>,
    pub success: bool,
}

#[derive(Debug, Clone)]
pub struct MultiStepAggregatedMetrics {
    pub runs: usize,
    pub steps: usize,
    /// Average bytes per step index, across successful runs only.
    pub avg_step_bytes: Vec<f64>,
    /// Running total of `avg_step_bytes` (cumulative cost after N steps).
    pub cumulative_avg_bytes: Vec<f64>,
    pub success_rate: f64,
}

/// Aggregate multi-step results into per-step and cumulative averages.
///
/// Only successful runs contribute to `avg_step_bytes`/`cumulative_avg_bytes`.
/// The step count is the shortest `step_bytes` among successful runs, so a
/// run that ended early (e.g. a mid-sequence navigation failure) can't cause
/// an out-of-bounds read.
pub fn aggregate_multi_step(results: &[MultiStepResult]) -> MultiStepAggregatedMetrics {
    let n = results.len();
    let ok: Vec<&MultiStepResult> = results.iter().filter(|r| r.success).collect();

    let steps = ok.iter().map(|r| r.step_bytes.len()).min().unwrap_or(0);

    let avg_step_bytes: Vec<f64> = (0..steps)
        .map(|i| {
            let sum: usize = ok.iter().map(|r| r.step_bytes[i]).sum();
            sum as f64 / ok.len() as f64
        })
        .collect();

    let mut cumulative_avg_bytes = Vec::with_capacity(avg_step_bytes.len());
    let mut running = 0.0;
    for bytes in &avg_step_bytes {
        running += bytes;
        cumulative_avg_bytes.push(running);
    }

    MultiStepAggregatedMetrics {
        runs: n,
        steps,
        avg_step_bytes,
        cumulative_avg_bytes,
        success_rate: if n > 0 {
            ok.len() as f64 / n as f64 * 100.0
        } else {
            0.0
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_tokens_divides_by_four() {
        assert_eq!(estimate_tokens(4000), 1000);
        assert_eq!(estimate_tokens(0), 0);
        assert_eq!(estimate_tokens(7), 1); // integer division
    }

    #[test]
    fn cost_savings_95_percent_reduction() {
        let savings = cost_savings(10_000, 500);
        assert!((savings.token_reduction_pct - 95.0).abs() < 0.01);
        let expected_gpt4o = 9_500.0 * GPT4O_COST_PER_TOKEN;
        assert!((savings.gpt4o_savings_usd - expected_gpt4o).abs() < 1e-9);
        let expected_claude = 9_500.0 * CLAUDE_COST_PER_TOKEN;
        assert!((savings.claude_savings_usd - expected_claude).abs() < 1e-9);
    }

    #[test]
    fn cost_savings_zero_raw_tokens() {
        let savings = cost_savings(0, 0);
        assert_eq!(savings.token_reduction_pct, 0.0);
        assert_eq!(savings.gpt4o_savings_usd, 0.0);
    }

    #[test]
    fn aggregate_computes_averages() {
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
        let m = aggregate(&results);
        assert_eq!(m.runs, 2);
        // avg raw tokens from 2 successful raw runs: (2000 + 3000) / 2 = 2500
        assert_eq!(m.raw_avg_tokens, 2500);
        // avg sre tokens from 1 successful sre run: 100
        assert_eq!(m.sre_avg_tokens, 100);
        // avg raw ttft from 2 successful raw runs: (100 + 200) / 2 = 150.0
        assert!((m.raw_avg_ttft_ms - 150.0).abs() < 0.01);
        // avg sre ttft from 1 successful sre run: 50.0
        assert!((m.sre_avg_ttft_ms - 50.0).abs() < 0.01);
        // success rates
        assert!((m.raw_success_rate - 100.0).abs() < 0.01);
        assert!((m.sre_success_rate - 50.0).abs() < 0.01);
    }

    #[test]
    fn aggregate_failed_runs_excluded_from_token_averages() {
        let results = vec![
            RunResult {
                run: 0,
                raw_html_bytes: 0, // failed
                sre_bytes: 0,      // failed
                raw_html_ttft_ms: 0,
                sre_ttft_ms: 0,
                raw_success: false,
                sre_success: false,
            },
            RunResult {
                run: 1,
                raw_html_bytes: 8000,
                sre_bytes: 400,
                raw_html_ttft_ms: 100,
                sre_ttft_ms: 50,
                raw_success: true,
                sre_success: true,
            },
        ];
        let m = aggregate(&results);
        // Only the successful run contributes to token/latency averages
        assert_eq!(m.raw_avg_tokens, 2000); // 8000/4
        assert_eq!(m.sre_avg_tokens, 100); // 400/4
        assert_eq!(m.runs, 2);
        assert!((m.raw_success_rate - 50.0).abs() < 0.01);
    }

    #[test]
    fn cost_savings_negative_when_sre_larger() {
        // SRE produces more tokens than raw DOM — should show negative savings
        let savings = cost_savings(100, 200);
        assert!(savings.token_reduction_pct < 0.0);
        assert!(savings.gpt4o_savings_usd < 0.0);
        assert!(savings.claude_savings_usd < 0.0);
    }

    #[test]
    fn aggregate_empty_returns_zero_metrics() {
        let m = aggregate(&[]);
        assert_eq!(m.runs, 0);
        assert_eq!(m.raw_avg_tokens, 0);
    }

    fn ok_multi_step(run: u32, step_bytes: Vec<usize>) -> MultiStepResult {
        let step_kinds = step_bytes.iter().map(|_| "delta").collect();
        MultiStepResult {
            run,
            step_bytes,
            step_kinds,
            success: true,
        }
    }

    #[test]
    fn aggregate_multi_step_computes_cumulative_and_avg() {
        let results = vec![
            ok_multi_step(0, vec![100, 20, 15]),
            ok_multi_step(1, vec![120, 30, 10]),
        ];
        let m = aggregate_multi_step(&results);
        assert_eq!(m.runs, 2);
        assert_eq!(m.steps, 3);
        assert_eq!(m.avg_step_bytes, vec![110.0, 25.0, 12.5]);
        assert_eq!(m.cumulative_avg_bytes, vec![110.0, 135.0, 147.5]);
        assert!((m.success_rate - 100.0).abs() < 0.01);
    }

    #[test]
    fn aggregate_multi_step_excludes_failed_runs_from_step_averages() {
        let mut failed = ok_multi_step(0, vec![999, 999, 999]);
        failed.success = false;
        let results = vec![failed, ok_multi_step(1, vec![100, 20, 15])];
        let m = aggregate_multi_step(&results);
        assert_eq!(m.runs, 2);
        assert_eq!(m.avg_step_bytes, vec![100.0, 20.0, 15.0]);
        assert!((m.success_rate - 50.0).abs() < 0.01);
    }

    #[test]
    fn aggregate_multi_step_empty_returns_zero_metrics() {
        let m = aggregate_multi_step(&[]);
        assert_eq!(m.runs, 0);
        assert_eq!(m.steps, 0);
        assert!(m.avg_step_bytes.is_empty());
        assert!(m.cumulative_avg_bytes.is_empty());
        assert_eq!(m.success_rate, 0.0);
    }

    #[test]
    fn aggregate_multi_step_single_step_run() {
        let results = vec![ok_multi_step(0, vec![500])];
        let m = aggregate_multi_step(&results);
        assert_eq!(m.steps, 1);
        assert_eq!(m.avg_step_bytes, vec![500.0]);
        assert_eq!(m.cumulative_avg_bytes, vec![500.0]);
    }

    #[test]
    fn aggregate_multi_step_zero_length_step_bytes_does_not_panic() {
        // A successful run with no recorded steps (e.g. all steps were
        // no-ops) must shrink the shared step count rather than panic on an
        // out-of-bounds index into the shorter vector.
        let results = vec![ok_multi_step(0, vec![]), ok_multi_step(1, vec![100, 20])];
        let m = aggregate_multi_step(&results);
        assert_eq!(m.steps, 0);
        assert!(m.avg_step_bytes.is_empty());
        assert!(m.cumulative_avg_bytes.is_empty());
    }

    #[test]
    fn aggregate_multi_step_all_runs_failed() {
        let mut r0 = ok_multi_step(0, vec![100, 20]);
        r0.success = false;
        let mut r1 = ok_multi_step(1, vec![100, 20]);
        r1.success = false;
        let m = aggregate_multi_step(&[r0, r1]);
        assert_eq!(m.runs, 2);
        assert_eq!(m.steps, 0);
        assert!(m.avg_step_bytes.is_empty());
        assert_eq!(m.success_rate, 0.0);
    }
}
