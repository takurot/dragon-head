import { reductionPct, costUsd, GPT4O_COST_PER_MILLION, type PlaywrightMetrics } from './metrics.js';

export function buildMarkdownReport(metricsList: PlaywrightMetrics[]): string {
  const runs = metricsList[0]?.runs ?? 0;
  const lines: string[] = [
    '# Playwright Benchmark Report\n',
    `**Runs per scenario:** ${runs}  `,
    `**Generated:** ${new Date().toISOString()}\n`,
  ];

  for (const m of metricsList) {
    const customReduction = reductionPct(m.raw_html.avg_tokens, m.custom_extract.avg_tokens);

    lines.push(`## ${m.url}\n`);
    lines.push('| Approach | Avg Tokens | Avg TTFT (ms) | Success Rate | GPT-4o Cost/call |');
    lines.push('|---|---:|---:|---:|---:|');
    lines.push(
      `| \`page.content()\` (raw HTML) | ${m.raw_html.avg_tokens} | ${m.raw_html.avg_ttft_ms.toFixed(1)} | ${m.raw_html.success_rate.toFixed(0)}% | $${costUsd(m.raw_html.avg_tokens, GPT4O_COST_PER_MILLION).toFixed(6)} |`,
    );
    lines.push(
      `| custom extract (interactive only) | ${m.custom_extract.avg_tokens} | — | ${m.custom_extract.success_rate.toFixed(0)}% | $${costUsd(m.custom_extract.avg_tokens, GPT4O_COST_PER_MILLION).toFixed(6)} |`,
    );
    lines.push(
      `| screenshot (PNG) | ${(m.screenshot.avg_bytes / 1024).toFixed(0)} KB | — | ${m.screenshot.success_rate.toFixed(0)}% | — |`,
    );
    lines.push('');
    lines.push(
      `**Custom extract reduces tokens by ${customReduction.toFixed(1)}% vs raw HTML.**\n`,
    );
  }

  return lines.join('\n');
}
