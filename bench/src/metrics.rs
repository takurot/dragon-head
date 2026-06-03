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
}
