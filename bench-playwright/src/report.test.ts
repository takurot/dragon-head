import { describe, it, expect } from 'vitest';
import { buildMarkdownReport, buildMultiStepMarkdownReport } from './report.js';
import type { PlaywrightMetrics, MultiStepPlaywrightMetrics } from './metrics.js';

const sampleMetrics: PlaywrightMetrics = {
  url: 'https://example.com',
  runs: 3,
  raw_html: { avg_tokens: 10000, avg_ttft_ms: 350, success_rate: 100 },
  custom_extract: { avg_tokens: 1500, avg_ttft_ms: 352, success_rate: 100 },
  screenshot: { avg_bytes: 120000, success_rate: 100 },
};

describe('buildMarkdownReport', () => {
  it('includes report title', () => {
    const md = buildMarkdownReport([sampleMetrics]);
    expect(md).toContain('# Playwright Benchmark Report');
  });

  it('shows page.content() approach', () => {
    const md = buildMarkdownReport([sampleMetrics]);
    expect(md).toContain('page.content()');
  });

  it('shows custom extract approach', () => {
    const md = buildMarkdownReport([sampleMetrics]);
    expect(md).toContain('custom extract');
  });

  it('includes token counts', () => {
    const md = buildMarkdownReport([sampleMetrics]);
    expect(md).toContain('10000');
    expect(md).toContain('1500');
  });

  it('includes the URL', () => {
    const md = buildMarkdownReport([sampleMetrics]);
    expect(md).toContain('https://example.com');
  });

  it('includes reduction percentage', () => {
    const md = buildMarkdownReport([sampleMetrics]);
    // 10000 → 1500 = 85% reduction
    expect(md).toContain('85.0%');
  });

  it('handles multiple metrics', () => {
    const second: PlaywrightMetrics = { ...sampleMetrics, url: 'file://fixtures/form.html' };
    const md = buildMarkdownReport([sampleMetrics, second]);
    expect(md).toContain('https://example.com');
    expect(md).toContain('file://fixtures/form.html');
  });
});

const sampleMultiStepMetrics: MultiStepPlaywrightMetrics = {
  name: 'spa-filter-cycle',
  url: 'file:///fixtures/spa-like.html',
  runs: 2,
  raw_html: {
    runs: 2,
    steps: 2,
    avg_step_bytes: [20000, 20100],
    cumulative_avg_bytes: [20000, 40100],
    success_rate: 100,
  },
  custom_extract: {
    runs: 2,
    steps: 2,
    avg_step_bytes: [5000, 5010],
    cumulative_avg_bytes: [5000, 10010],
    success_rate: 100,
  },
};

describe('buildMultiStepMarkdownReport', () => {
  it('includes report title and scenario name', () => {
    const md = buildMultiStepMarkdownReport([sampleMultiStepMetrics]);
    expect(md).toContain('# Playwright Multi-Step Cumulative Cost Report');
    expect(md).toContain('spa-filter-cycle');
  });

  it('shows cumulative bytes per step for both approaches', () => {
    const md = buildMultiStepMarkdownReport([sampleMultiStepMetrics]);
    expect(md).toContain('40100');
    expect(md).toContain('10010');
  });

  it('notes Playwright has no delta-delivery concept', () => {
    const md = buildMultiStepMarkdownReport([sampleMultiStepMetrics]);
    expect(md).toContain('no delta-delivery concept');
  });

  it('does not throw and bounds the table when custom_extract has fewer steps than raw_html (ISSUE-278)', () => {
    const mismatched: MultiStepPlaywrightMetrics = {
      ...sampleMultiStepMetrics,
      raw_html: {
        runs: 2,
        steps: 3,
        avg_step_bytes: [20000, 20100, 20200],
        cumulative_avg_bytes: [20000, 40100, 60300],
        success_rate: 100,
      },
      custom_extract: {
        runs: 2,
        steps: 1,
        avg_step_bytes: [5000],
        cumulative_avg_bytes: [5000],
        success_rate: 50,
      },
    };
    expect(() => buildMultiStepMarkdownReport([mismatched])).not.toThrow();
    const md = buildMultiStepMarkdownReport([mismatched]);
    // Only the first (shared) step should appear in the table.
    expect(md).toContain('5000');
    expect(md).not.toContain('40100');
    expect(md).toContain('**Steps:** 1');
    expect(md).toContain('raw HTML captured 3 step(s) but custom extract only 1');
  });
});
