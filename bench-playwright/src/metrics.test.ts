import { describe, it, expect } from 'vitest';
import {
  estimateTokens,
  reductionPct,
  costUsd,
  aggregateResults,
  GPT4O_COST_PER_MILLION,
  CLAUDE_COST_PER_MILLION,
  type RunResult,
} from './metrics.js';

describe('estimateTokens', () => {
  it('divides bytes by 4', () => {
    expect(estimateTokens(4000)).toBe(1000);
    expect(estimateTokens(0)).toBe(0);
  });

  it('floors fractional tokens', () => {
    expect(estimateTokens(7)).toBe(1);
    expect(estimateTokens(3)).toBe(0);
  });
});

describe('reductionPct', () => {
  it('computes percentage reduction from baseline', () => {
    expect(reductionPct(10000, 500)).toBeCloseTo(95.0);
    expect(reductionPct(10000, 1000)).toBeCloseTo(90.0);
    expect(reductionPct(10000, 10000)).toBeCloseTo(0.0);
  });

  it('returns 0 when baseline is 0', () => {
    expect(reductionPct(0, 0)).toBe(0);
  });

  it('returns negative when value exceeds baseline', () => {
    expect(reductionPct(100, 200)).toBeCloseTo(-100.0);
  });
});

describe('costUsd', () => {
  it('computes USD at given price per million tokens', () => {
    expect(costUsd(1_000_000, GPT4O_COST_PER_MILLION)).toBeCloseTo(5.0);
    expect(costUsd(1_000_000, CLAUDE_COST_PER_MILLION)).toBeCloseTo(3.0);
  });

  it('scales linearly with token count', () => {
    expect(costUsd(500_000, 5.0)).toBeCloseTo(2.5);
    expect(costUsd(0, 5.0)).toBe(0);
  });
});

describe('aggregateResults', () => {
  const makeRun = (overrides: Partial<RunResult> = {}): RunResult => ({
    url: 'http://example.com',
    run_idx: 0,
    raw_html_bytes: 8000,
    custom_extract_bytes: 400,
    screenshot_bytes: 50000,
    ttft_ms: 100,
    raw_success: true,
    custom_success: true,
    screenshot_success: true,
    ...overrides,
  });

  it('computes token averages from successful runs only', () => {
    const results = [
      makeRun({ run_idx: 0, raw_html_bytes: 8000, custom_extract_bytes: 400 }),
      makeRun({ run_idx: 1, raw_html_bytes: 0, custom_extract_bytes: 0, raw_success: false, custom_success: false, screenshot_success: false }),
    ];
    const m = aggregateResults('http://example.com', results);
    expect(m.raw_html.avg_tokens).toBe(2000); // 8000/4 from 1 successful run
    expect(m.custom_extract.avg_tokens).toBe(100); // 400/4
  });

  it('computes success rates across all runs', () => {
    const results = [
      makeRun({ run_idx: 0 }),
      makeRun({ run_idx: 1, raw_success: false, custom_success: false, screenshot_success: false }),
    ];
    const m = aggregateResults('http://example.com', results);
    expect(m.raw_html.success_rate).toBeCloseTo(50.0);
    expect(m.custom_extract.success_rate).toBeCloseTo(50.0);
    expect(m.screenshot.success_rate).toBeCloseTo(50.0);
  });

  it('returns zero metrics for empty results', () => {
    const m = aggregateResults('http://example.com', []);
    expect(m.raw_html.avg_tokens).toBe(0);
    expect(m.runs).toBe(0);
  });

  it('averages multiple successful runs', () => {
    const results = [
      makeRun({ run_idx: 0, raw_html_bytes: 4000, ttft_ms: 100 }),
      makeRun({ run_idx: 1, raw_html_bytes: 8000, ttft_ms: 200 }),
    ];
    const m = aggregateResults('http://example.com', results);
    expect(m.raw_html.avg_tokens).toBe(1500); // (1000 + 2000) / 2
    expect(m.raw_html.avg_ttft_ms).toBeCloseTo(150.0);
    expect(m.raw_html.success_rate).toBeCloseTo(100.0);
  });
});
