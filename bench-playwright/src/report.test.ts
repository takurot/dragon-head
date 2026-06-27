import { describe, it, expect } from 'vitest';
import { buildMarkdownReport } from './report.js';
import type { PlaywrightMetrics } from './metrics.js';

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
