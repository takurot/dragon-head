import {
  reductionPct,
  costUsd,
  GPT4O_COST_PER_MILLION,
  type PlaywrightMetrics,
  type MultiStepPlaywrightMetrics,
} from './metrics.js';

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

/**
 * Cumulative multi-step comparison report (issue #173). Playwright has no
 * delta concept, so both approaches re-fetch the full payload every step —
 * this report is the "no reduction" control side, meant to be read next to
 * the `bench` crate's dragon-head delta-cost numbers for the same scenario.
 */
export function buildMultiStepMarkdownReport(metricsList: MultiStepPlaywrightMetrics[]): string {
  const lines: string[] = [
    '# Playwright Multi-Step Cumulative Cost Report\n',
    `**Generated:** ${new Date().toISOString()}\n`,
    '> Playwright has no delta-delivery concept: every step below re-fetches the full page payload. Compare against the dragon-head `bench` crate\'s per-step "delta"/"full"/"noop" breakdown for the same scenario.\n',
  ];

  for (const m of metricsList) {
    // The two approaches aggregate their step counts independently (each is
    // capped at the shortest successful run for *that* approach — see
    // aggregateMultiStepResults), so custom_extract.steps can be shorter
    // than raw_html.steps if extraction failed on a step raw HTML captured
    // fine. Bound the loop by the shorter of the two so every index is
    // valid in both arrays.
    const steps = Math.min(m.raw_html.steps, m.custom_extract.steps);
    lines.push(`## ${m.name} (${m.url})\n`);
    lines.push(`**Runs:** ${m.runs}  |  **Steps:** ${steps}\n`);
    if (m.raw_html.steps !== m.custom_extract.steps) {
      lines.push(
        `> Note: raw HTML captured ${m.raw_html.steps} step(s) but custom extract only ${m.custom_extract.steps} — showing the first ${steps}.\n`,
      );
    }
    lines.push('| Step | Raw HTML Bytes | Raw HTML Cumulative | Custom Extract Bytes | Custom Extract Cumulative |');
    lines.push('|-----:|---------------:|---------------------:|----------------------:|---------------------------:|');
    for (let i = 0; i < steps; i++) {
      lines.push(
        `| ${i} | ${m.raw_html.avg_step_bytes[i]!.toFixed(0)} | ${m.raw_html.cumulative_avg_bytes[i]!.toFixed(0)} | ${m.custom_extract.avg_step_bytes[i]!.toFixed(0)} | ${m.custom_extract.cumulative_avg_bytes[i]!.toFixed(0)} |`,
      );
    }
    lines.push('');
    const rawTotal = m.raw_html.cumulative_avg_bytes.at(-1) ?? 0;
    const customTotal = m.custom_extract.cumulative_avg_bytes.at(-1) ?? 0;
    lines.push(
      `**Total cumulative cost over ${m.raw_html.steps} steps:** raw HTML ${costUsd(rawTotal / 4, GPT4O_COST_PER_MILLION).toFixed(6)} USD, custom extract ${costUsd(customTotal / 4, GPT4O_COST_PER_MILLION).toFixed(6)} USD (GPT-4o pricing).\n`,
    );
  }

  return lines.join('\n');
}
