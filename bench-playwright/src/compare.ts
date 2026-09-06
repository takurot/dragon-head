import { readFileSync, writeFileSync, mkdirSync } from 'fs';
import { dirname } from 'path';
import { pathToFileURL } from 'url';
import { reductionPct, costUsd, GPT4O_COST_PER_MILLION, type PlaywrightMetrics } from './metrics.js';
import { argValue } from './cli-args.js';

/** Read and JSON.parse a metrics file, with a clear error instead of a raw stack trace. */
function readMetricsFile(path: string, label: string): unknown {
  let text: string;
  try {
    text = readFileSync(path, 'utf8');
  } catch (err) {
    console.error(`Failed to read ${label} metrics file "${path}": ${err instanceof Error ? err.message : err}`);
    process.exit(1);
  }
  try {
    return JSON.parse(text);
  } catch (err) {
    console.error(`Failed to parse ${label} metrics file "${path}" as JSON: ${err instanceof Error ? err.message : err}`);
    process.exit(1);
  }
}

export interface DragonHeadApproachMetrics {
  avg_tokens: number;
  avg_ttft_ms: number;
  success_rate: number;
}

export interface DragonHeadMetrics {
  url: string;
  runs: number;
  raw_html: DragonHeadApproachMetrics;
  sre_minimal: DragonHeadApproachMetrics;
  cost_savings: {
    token_reduction_pct: number;
    gpt4o_savings_usd: number;
    claude_savings_usd: number;
  };
}

export function buildComparisonMarkdown(
  pwList: PlaywrightMetrics[],
  dhList: DragonHeadMetrics[],
): string {
  const dhMap = new Map(dhList.map((d) => [d.url, d]));
  const lines: string[] = [
    '# Playwright vs Dragon-Head Comparison Report\n',
    `**Generated:** ${new Date().toISOString()}\n`,
    '> Token estimate: 1 token ≈ 4 bytes. Pricing: GPT-4o $5/1M tokens, Claude $3/1M tokens.\n',
  ];

  for (const pw of pwList) {
    const dh = dhMap.get(pw.url);
    const baseline = pw.raw_html.avg_tokens;

    lines.push(`## ${pw.url}\n`);
    lines.push('| Approach | Avg Tokens | vs Raw HTML | Avg TTFT | GPT-4o Cost/call |');
    lines.push('|---|---:|---:|---:|---:|');
    lines.push(
      `| Playwright: \`page.content()\` | ${baseline} | — | ${pw.raw_html.avg_ttft_ms.toFixed(0)} ms | $${costUsd(baseline, GPT4O_COST_PER_MILLION).toFixed(6)} |`,
    );
    lines.push(
      `| Playwright: custom extract | ${pw.custom_extract.avg_tokens} | ${reductionPct(baseline, pw.custom_extract.avg_tokens).toFixed(1)}% | — | $${costUsd(pw.custom_extract.avg_tokens, GPT4O_COST_PER_MILLION).toFixed(6)} |`,
    );

    if (dh) {
      lines.push(
        `| Dragon Head: SRE Minimal | ${dh.sre_minimal.avg_tokens} | ${reductionPct(baseline, dh.sre_minimal.avg_tokens).toFixed(1)}% | ${dh.sre_minimal.avg_ttft_ms.toFixed(0)} ms | $${costUsd(dh.sre_minimal.avg_tokens, GPT4O_COST_PER_MILLION).toFixed(6)} |`,
      );
    }

    lines.push('');

    if (dh) {
      lines.push(
        `**Dragon Head achieves ${dh.cost_savings.token_reduction_pct.toFixed(2)}% token reduction vs raw HTML.**`,
      );
      lines.push(`- GPT-4o savings per call: $${dh.cost_savings.gpt4o_savings_usd.toFixed(6)}`);
      lines.push(`- Claude savings per call: $${dh.cost_savings.claude_savings_usd.toFixed(6)}`);
      lines.push('');
    }
  }

  return lines.join('\n');
}

// CLI entry point. Compares this module's URL against the invoked script's
// path (the standard ESM idiom for "is this the entry module", replacing
// CommonJS's `require.main === module`) rather than `endsWith('compare.ts')`
// — a fragile check that breaks under a symlink, a renamed copy, or any
// invocation path that doesn't literally end in that filename.
if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  const args = process.argv.slice(2);
  const pwPath = argValue(args, '--playwright');
  const dhPath = argValue(args, '--dragon-head');

  if (!pwPath || !dhPath) {
    console.error(
      'Usage: tsx src/compare.ts --playwright=<path> --dragon-head=<path> [--output=<path>]',
    );
    process.exit(1);
  }

  const pwData = readMetricsFile(pwPath, 'Playwright') as PlaywrightMetrics[];
  const dhData = readMetricsFile(dhPath, 'Dragon Head') as DragonHeadMetrics[];
  const md = buildComparisonMarkdown(pwData, dhData);

  const outputPath = argValue(args, '--output') ?? 'comparison-report.md';
  const dir = dirname(outputPath);
  if (dir && dir !== '.') mkdirSync(dir, { recursive: true });
  writeFileSync(outputPath, md);
  console.log(`Comparison report written to ${outputPath}`);
}
