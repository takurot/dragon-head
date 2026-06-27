import { describe, it, expect } from 'vitest';
import { buildComparisonMarkdown } from './compare.js';
import type { PlaywrightMetrics } from './metrics.js';
import type { DragonHeadMetrics } from './compare.js';

const pw: PlaywrightMetrics = {
  url: 'https://example.com',
  runs: 3,
  raw_html: { avg_tokens: 10000, avg_ttft_ms: 350, success_rate: 100 },
  custom_extract: { avg_tokens: 1500, avg_ttft_ms: 352, success_rate: 100 },
  screenshot: { avg_bytes: 120000, success_rate: 100 },
};

const dh: DragonHeadMetrics = {
  url: 'https://example.com',
  runs: 3,
  raw_html: { avg_tokens: 9800, avg_ttft_ms: 355, success_rate: 100 },
  sre_minimal: { avg_tokens: 125, avg_ttft_ms: 90, success_rate: 100 },
  cost_savings: {
    token_reduction_pct: 98.75,
    gpt4o_savings_usd: 0.048875,
    claude_savings_usd: 0.029325,
  },
};

describe('buildComparisonMarkdown', () => {
  it('includes comparison report title', () => {
    const md = buildComparisonMarkdown([pw], [dh]);
    expect(md).toContain('# Playwright vs Dragon-Head Comparison Report');
  });

  it('shows Playwright raw HTML row', () => {
    const md = buildComparisonMarkdown([pw], [dh]);
    expect(md).toContain('page.content()');
    expect(md).toContain('10000');
  });

  it('shows Playwright custom extract row', () => {
    const md = buildComparisonMarkdown([pw], [dh]);
    expect(md).toContain('custom extract');
    expect(md).toContain('1500');
  });

  it('shows Dragon Head SRE row', () => {
    const md = buildComparisonMarkdown([pw], [dh]);
    expect(md).toContain('Dragon Head: SRE');
    expect(md).toContain('125');
  });

  it('shows token reduction percentage', () => {
    const md = buildComparisonMarkdown([pw], [dh]);
    expect(md).toContain('98.75%');
  });

  it('shows cost savings', () => {
    const md = buildComparisonMarkdown([pw], [dh]);
    expect(md).toContain('0.048875');
  });

  it('works when dragon-head data is absent for a scenario', () => {
    const md = buildComparisonMarkdown([pw], []);
    expect(md).toContain('page.content()');
    expect(md).not.toContain('Dragon Head');
  });
});
