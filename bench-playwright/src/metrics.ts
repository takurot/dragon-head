export const GPT4O_COST_PER_MILLION = 5.0;
export const CLAUDE_COST_PER_MILLION = 3.0;

export interface RunResult {
  url: string;
  run_idx: number;
  raw_html_bytes: number;
  custom_extract_bytes: number;
  screenshot_bytes: number;
  ttft_ms: number;
  raw_success: boolean;
  custom_success: boolean;
  screenshot_success: boolean;
}

export interface ApproachMetrics {
  avg_tokens: number;
  avg_ttft_ms: number;
  success_rate: number;
}

export interface PlaywrightMetrics {
  url: string;
  runs: number;
  raw_html: ApproachMetrics;
  custom_extract: ApproachMetrics;
  screenshot: { avg_bytes: number; success_rate: number };
}

export function estimateTokens(bytes: number): number {
  return Math.floor(bytes / 4);
}

export function reductionPct(baseline: number, value: number): number {
  if (baseline === 0) return 0;
  return (1 - value / baseline) * 100;
}

export function costUsd(tokens: number, pricePerMillion: number): number {
  return (tokens / 1_000_000) * pricePerMillion;
}

function avg(values: number[]): number {
  if (values.length === 0) return 0;
  return values.reduce((a, b) => a + b, 0) / values.length;
}

export function aggregateResults(url: string, results: RunResult[]): PlaywrightMetrics {
  const n = results.length;
  const rawOk = results.filter((r) => r.raw_success);
  const customOk = results.filter((r) => r.custom_success);
  const shotOk = results.filter((r) => r.screenshot_success);

  return {
    url,
    runs: n,
    raw_html: {
      avg_tokens: Math.floor(avg(rawOk.map((r) => estimateTokens(r.raw_html_bytes)))),
      avg_ttft_ms: avg(rawOk.map((r) => r.ttft_ms)),
      success_rate: n > 0 ? (rawOk.length / n) * 100 : 0,
    },
    custom_extract: {
      avg_tokens: Math.floor(avg(customOk.map((r) => estimateTokens(r.custom_extract_bytes)))),
      avg_ttft_ms: avg(customOk.map((r) => r.ttft_ms)),
      success_rate: n > 0 ? (customOk.length / n) * 100 : 0,
    },
    screenshot: {
      avg_bytes: Math.floor(avg(shotOk.map((r) => r.screenshot_bytes))),
      success_rate: n > 0 ? (shotOk.length / n) * 100 : 0,
    },
  };
}
