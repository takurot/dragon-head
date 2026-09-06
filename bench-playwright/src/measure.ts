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
import { argValue } from './cli-args.js';

const DEFAULT_RUNS = 3;

/** Write the JSON results file (and an optional markdown report) and log both paths. */
function persistResults(outputJson: string, data: unknown, outputMd: string | undefined, markdown: string | undefined): void {
  mkdirSync(dirname(outputJson), { recursive: true });
  writeFileSync(outputJson, JSON.stringify(data, null, 2));
  console.log(`\nJSON results written to ${outputJson}`);

  if (outputMd && markdown !== undefined) {
    writeFileSync(outputMd, markdown);
    console.log(`Markdown report written to ${outputMd}`);
  }
}

/** Parse --runs=N, falling back to DEFAULT_RUNS on missing/non-numeric/non-positive input. */
function parseRuns(raw: string | undefined): number {
  if (raw === undefined) return DEFAULT_RUNS;
  const n = Number(raw);
  if (!Number.isInteger(n) || n <= 0) {
    console.error(`--runs=${raw} is not a positive integer; using default (${DEFAULT_RUNS}).`);
    return DEFAULT_RUNS;
  }
  return n;
}

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

  // The outer try/finally guarantees browser.close() runs even if a run
  // throws before reaching its own try (e.g. browser.newPage() itself
  // fails) — without it, a failure there would leak the browser process.
  try {
    for (let i = 0; i < runs; i++) {
      let page: Page | undefined;
      let raw_html_bytes = 0;
      let custom_extract_bytes = 0;
      let screenshot_bytes = 0;
      let raw_success = false;
      let custom_success = false;
      let screenshot_success = false;
      let ttft_ms = 0;

      try {
        page = await browser.newPage();
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
        await page?.close();
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
  } finally {
    await browser.close();
  }

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

  try {
    for (let i = 0; i < runs; i++) {
      let page: Page | undefined;
      const rawStepBytes: number[] = [];
      const customStepBytes: number[] = [];
      let success = true;

      try {
        page = await browser.newPage();
        const measureStep = async () => {
          const html = await page!.content();
          rawStepBytes.push(Buffer.byteLength(html, 'utf8'));
          customStepBytes.push(await extractInteractiveElementsBytes(page!));
        };

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
        await page?.close();
      }

      raw.push({ run_idx: i, step_bytes: rawStepBytes, success });
      custom.push({ run_idx: i, step_bytes: customStepBytes, success });
    }
  } finally {
    await browser.close();
  }

  return { raw, custom };
}

// CLI
const args = process.argv.slice(2);
const runs = parseRuns(argValue(args, '--runs'));
const outputMd = argValue(args, '--output-md');
const scenarioFilter = argValue(args, '--scenarios')?.split(',');
const multiStep = args.includes('--multi-step');

if (multiStep) {
  const outputJson = argValue(args, '--output') ?? 'results/playwright-multi-step.json';
  const activeMultiStep = scenarioFilter
    ? MULTI_STEP_SCENARIOS.filter((s) => scenarioFilter.includes(s.name))
    : MULTI_STEP_SCENARIOS;

  console.log(`Running ${activeMultiStep.length} multi-step scenario(s), ${runs} run(s) each...\n`);

  const allMultiStepMetrics: MultiStepPlaywrightMetrics[] = [];
  const skippedScenarios: string[] = [];
  for (const scenario of activeMultiStep) {
    console.log(`[${scenario.name}] ${scenario.url} (${scenario.stepSelectors.length} steps)`);
    // A scenario-level failure (e.g. the launched browser itself crashes)
    // shouldn't abort every scenario after it — report it and move on, same
    // as a single run's failure is already isolated inside measureMultiStepScenario.
    // The skip is still tracked (see skippedScenarios below) so the process
    // exit code reflects an incomplete result set instead of reporting
    // success on a partial JSON output.
    try {
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
    } catch (err) {
      console.error(`  [${scenario.name}] scenario failed, skipping: ${err}`);
      skippedScenarios.push(scenario.name);
    }
  }

  persistResults(
    outputJson,
    allMultiStepMetrics,
    outputMd,
    outputMd ? buildMultiStepMarkdownReport(allMultiStepMetrics) : undefined,
  );

  if (skippedScenarios.length > 0) {
    console.error(`\n${skippedScenarios.length} scenario(s) skipped: ${skippedScenarios.join(', ')}`);
    process.exitCode = 1;
  }
} else {
  const outputJson = argValue(args, '--output') ?? 'results/playwright-metrics.json';
  const active = scenarioFilter
    ? SCENARIOS.filter((s) => scenarioFilter.includes(s.name))
    : SCENARIOS;

  console.log(`Running ${active.length} scenario(s), ${runs} run(s) each...\n`);

  const allMetrics: PlaywrightMetrics[] = [];
  const skippedScenarios: string[] = [];
  for (const scenario of active) {
    console.log(`[${scenario.name}] ${scenario.url}`);
    // Same scenario-level isolation as the multi-step branch above.
    try {
      const results = await measureUrl(scenario.url, runs);
      const m = aggregateResults(scenario.url, results);
      allMetrics.push(m);
      console.log(`  raw HTML:       ${m.raw_html.avg_tokens} tokens  (${m.raw_html.avg_ttft_ms.toFixed(0)} ms TTFT)`);
      console.log(`  custom extract: ${m.custom_extract.avg_tokens} tokens`);
      console.log(`  screenshot:     ${(m.screenshot.avg_bytes / 1024).toFixed(0)} KB`);
    } catch (err) {
      console.error(`  [${scenario.name}] scenario failed, skipping: ${err}`);
      skippedScenarios.push(scenario.name);
    }
  }

  persistResults(outputJson, allMetrics, outputMd, outputMd ? buildMarkdownReport(allMetrics) : undefined);

  if (skippedScenarios.length > 0) {
    console.error(`\n${skippedScenarios.length} scenario(s) skipped: ${skippedScenarios.join(', ')}`);
    process.exitCode = 1;
  }
}
