import { chromium } from 'playwright';
import { writeFileSync, mkdirSync } from 'fs';
import { dirname } from 'path';
import {
  aggregateResults,
  type RunResult,
  type PlaywrightMetrics,
} from './metrics.js';
import { SCENARIOS } from './scenarios.js';
import { buildMarkdownReport } from './report.js';

async function measureUrl(url: string, runs: number): Promise<RunResult[]> {
  const browser = await chromium.launch({ headless: true });
  const results: RunResult[] = [];

  for (let i = 0; i < runs; i++) {
    const page = await browser.newPage();
    let raw_html_bytes = 0;
    let custom_extract_bytes = 0;
    let screenshot_bytes = 0;
    let raw_success = false;
    let custom_success = false;
    let screenshot_success = false;
    let ttft_ms = 0;

    try {
      const t0 = Date.now();
      await page.goto(url, { waitUntil: 'networkidle', timeout: 30_000 });
      ttft_ms = Date.now() - t0;

      const html = await page.content();
      raw_html_bytes = Buffer.byteLength(html, 'utf8');
      raw_success = true;

      // Extract only interactive elements — mirrors what browser-use and similar
      // LLM browser integrations do to reduce context size.
      const elements = await page.evaluate(() =>
        Array.from(
          document.querySelectorAll('button, a[href], input, select, textarea'),
        ).map((el) => ({
          role: el.tagName.toLowerCase(),
          text: el.textContent?.trim().slice(0, 100) ?? '',
          type: el.getAttribute('type') ?? undefined,
          href: el.getAttribute('href') ?? undefined,
          placeholder: el.getAttribute('placeholder') ?? undefined,
          name: el.getAttribute('name') ?? undefined,
          ariaLabel: el.getAttribute('aria-label') ?? undefined,
        })),
      );
      custom_extract_bytes = Buffer.byteLength(JSON.stringify(elements), 'utf8');
      custom_success = true;

      const screenshot = await page.screenshot({ type: 'png' });
      screenshot_bytes = screenshot.length;
      screenshot_success = true;
    } catch (err) {
      console.error(`  Run ${i + 1} failed: ${err}`);
    } finally {
      await page.close();
    }

    results.push({
      url,
      run_idx: i,
      raw_html_bytes,
      custom_extract_bytes,
      screenshot_bytes,
      ttft_ms,
      raw_success,
      custom_success,
      screenshot_success,
    });
  }

  await browser.close();
  return results;
}

// CLI
const args = process.argv.slice(2);
const runs = Number(args.find((a) => a.startsWith('--runs='))?.split('=')[1] ?? 3);
const outputJson = args.find((a) => a.startsWith('--output='))?.split('=')[1] ?? 'results/playwright-metrics.json';
const outputMd = args.find((a) => a.startsWith('--output-md='))?.split('=')[1];
const scenarioFilter = args.find((a) => a.startsWith('--scenarios='))?.split('=')[1]?.split(',');

const active = scenarioFilter
  ? SCENARIOS.filter((s) => scenarioFilter.includes(s.name))
  : SCENARIOS;

console.log(`Running ${active.length} scenario(s), ${runs} run(s) each...\n`);

const allMetrics: PlaywrightMetrics[] = [];
for (const scenario of active) {
  console.log(`[${scenario.name}] ${scenario.url}`);
  const results = await measureUrl(scenario.url, runs);
  const m = aggregateResults(scenario.url, results);
  allMetrics.push(m);
  console.log(`  raw HTML:       ${m.raw_html.avg_tokens} tokens  (${m.raw_html.avg_ttft_ms.toFixed(0)} ms TTFT)`);
  console.log(`  custom extract: ${m.custom_extract.avg_tokens} tokens`);
  console.log(`  screenshot:     ${(m.screenshot.avg_bytes / 1024).toFixed(0)} KB`);
}

mkdirSync(dirname(outputJson), { recursive: true });
writeFileSync(outputJson, JSON.stringify(allMetrics, null, 2));
console.log(`\nJSON results written to ${outputJson}`);

if (outputMd) {
  const md = buildMarkdownReport(allMetrics);
  writeFileSync(outputMd, md);
  console.log(`Markdown report written to ${outputMd}`);
}
