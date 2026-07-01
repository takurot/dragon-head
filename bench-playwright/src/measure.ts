import { chromium, type Page } from 'playwright';
import { writeFileSync, mkdirSync } from 'fs';
import { dirname } from 'path';
import {
  aggregateResults,
  aggregateMultiStepResults,
  type RunResult,
  type PlaywrightMetrics,
  type MultiStepRunResult,
  type MultiStepPlaywrightMetrics,
} from './metrics.js';
import { SCENARIOS, MULTI_STEP_SCENARIOS, type MultiStepScenario } from './scenarios.js';
import { buildMarkdownReport, buildMultiStepMarkdownReport } from './report.js';

// Extract only interactive elements — mirrors what browser-use and similar
// LLM browser integrations do to reduce context size. Shared between the
// single-call and multi-step measurements so both use the same
// representation (see docs/bench-playwright-comparison.md#issue-173).
async function extractInteractiveElementsBytes(page: Page): Promise<number> {
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
  return Buffer.byteLength(JSON.stringify(elements), 'utf8');
}

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

      custom_extract_bytes = await extractInteractiveElementsBytes(page);
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

/**
 * Measure cumulative token cost of a multi-step interaction sequence
 * (issue #173). Navigates once, then re-measures both Playwright approaches
 * (raw `page.content()` and the custom interactive-elements extraction)
 * after every click — Playwright has no delta concept, so every step is a
 * full re-fetch. This is the "no reduction" control side of the comparison
 * against dragon-head's delta-based `bench` crate numbers.
 */
async function measureMultiStepScenario(
  scenario: MultiStepScenario,
  runs: number,
): Promise<{ raw: MultiStepRunResult[]; custom: MultiStepRunResult[] }> {
  const browser = await chromium.launch({ headless: true });
  const raw: MultiStepRunResult[] = [];
  const custom: MultiStepRunResult[] = [];

  for (let i = 0; i < runs; i++) {
    const page = await browser.newPage();
    const rawStepBytes: number[] = [];
    const customStepBytes: number[] = [];
    let success = true;

    const measureStep = async () => {
      const html = await page.content();
      rawStepBytes.push(Buffer.byteLength(html, 'utf8'));
      customStepBytes.push(await extractInteractiveElementsBytes(page));
    };

    try {
      await page.goto(scenario.url, { waitUntil: 'networkidle', timeout: 30_000 });
      await measureStep(); // step 0: initial full capture

      for (const selector of scenario.stepSelectors) {
        await page.click(selector, { timeout: 5_000 });
        await measureStep();
      }
    } catch (err) {
      console.error(`  Run ${i + 1} failed: ${err}`);
      success = false;
    } finally {
      await page.close();
    }

    raw.push({ run_idx: i, step_bytes: rawStepBytes, success });
    custom.push({ run_idx: i, step_bytes: customStepBytes, success });
  }

  await browser.close();
  return { raw, custom };
}

// CLI
const args = process.argv.slice(2);
const runs = Number(args.find((a) => a.startsWith('--runs='))?.split('=')[1] ?? 3);
const outputMd = args.find((a) => a.startsWith('--output-md='))?.split('=')[1];
const scenarioFilter = args.find((a) => a.startsWith('--scenarios='))?.split('=')[1]?.split(',');
const multiStep = args.includes('--multi-step');

if (multiStep) {
  const outputJson =
    args.find((a) => a.startsWith('--output='))?.split('=')[1] ?? 'results/playwright-multi-step.json';
  const activeMultiStep = scenarioFilter
    ? MULTI_STEP_SCENARIOS.filter((s) => scenarioFilter.includes(s.name))
    : MULTI_STEP_SCENARIOS;

  console.log(`Running ${activeMultiStep.length} multi-step scenario(s), ${runs} run(s) each...\n`);

  const allMultiStepMetrics: MultiStepPlaywrightMetrics[] = [];
  for (const scenario of activeMultiStep) {
    console.log(`[${scenario.name}] ${scenario.url} (${scenario.stepSelectors.length} steps)`);
    const { raw, custom } = await measureMultiStepScenario(scenario, runs);
    const m: MultiStepPlaywrightMetrics = {
      name: scenario.name,
      url: scenario.url,
      runs,
      raw_html: aggregateMultiStepResults(raw),
      custom_extract: aggregateMultiStepResults(custom),
    };
    allMultiStepMetrics.push(m);
    console.log(
      `  raw HTML cumulative:       ${m.raw_html.cumulative_avg_bytes.at(-1)?.toFixed(0) ?? 0} bytes`,
    );
    console.log(
      `  custom extract cumulative: ${m.custom_extract.cumulative_avg_bytes.at(-1)?.toFixed(0) ?? 0} bytes`,
    );
  }

  mkdirSync(dirname(outputJson), { recursive: true });
  writeFileSync(outputJson, JSON.stringify(allMultiStepMetrics, null, 2));
  console.log(`\nJSON results written to ${outputJson}`);

  if (outputMd) {
    const md = buildMultiStepMarkdownReport(allMultiStepMetrics);
    writeFileSync(outputMd, md);
    console.log(`Markdown report written to ${outputMd}`);
  }
} else {
  const outputJson =
    args.find((a) => a.startsWith('--output='))?.split('=')[1] ?? 'results/playwright-metrics.json';
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
}
