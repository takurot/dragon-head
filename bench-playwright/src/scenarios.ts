import { resolve, dirname } from 'path';
import { fileURLToPath } from 'url';

const fixturesDir = resolve(dirname(fileURLToPath(import.meta.url)), '..', 'fixtures');

// Deliberately a real external site, not another local fixture: this
// scenario exists specifically to confirm the harness behaves the same
// against a real network target as it does against file:// fixtures (the
// "baseline compatibility" the description below refers to). Overridable
// for environments where reaching the public internet during a benchmark
// run isn't desirable.
//
// The override is validated, not just substituted: measureUrl() catches
// page.goto() failures per-run rather than propagating them, so an
// empty/whitespace/malformed BENCH_EXTERNAL_URL wouldn't fail loudly — it
// would silently produce a scenario with all-zero, all-failed metrics that
// still looks like a normal (successful, exit-0) benchmark result.
const DEFAULT_EXTERNAL_BASELINE_URL = 'https://example.com';

export function resolveExternalBaselineUrl(raw: string | undefined): string {
  const trimmed = raw?.trim();
  if (!trimmed) return DEFAULT_EXTERNAL_BASELINE_URL;
  try {
    return new URL(trimmed).href;
  } catch {
    console.error(
      `BENCH_EXTERNAL_URL="${raw}" is not a valid URL; falling back to ${DEFAULT_EXTERNAL_BASELINE_URL}`,
    );
    return DEFAULT_EXTERNAL_BASELINE_URL;
  }
}

const EXTERNAL_BASELINE_URL = resolveExternalBaselineUrl(process.env.BENCH_EXTERNAL_URL);

export interface Scenario {
  name: string;
  url: string;
  description: string;
}

export const SCENARIOS: Scenario[] = [
  {
    name: 'simple',
    url: `file://${fixturesDir}/simple.html`,
    description: 'Static page with navigation and interactive elements',
  },
  {
    name: 'form',
    url: `file://${fixturesDir}/form.html`,
    description: 'Form-heavy page (login / checkout)',
  },
  {
    name: 'spa-like',
    url: `file://${fixturesDir}/spa-like.html`,
    description: 'SPA-like page with dynamically rendered content',
  },
  {
    name: 'example-com',
    url: EXTERNAL_BASELINE_URL,
    description: 'Real external site — baseline compatibility with Rust bench',
  },
];

/** A multi-step interaction sequence for cumulative delta-cost measurement (issue #173). */
export interface MultiStepScenario {
  name: string;
  url: string;
  description: string;
  /** CSS selectors clicked in sequence, one per interaction step. */
  stepSelectors: string[];
}

export const MULTI_STEP_SCENARIOS: MultiStepScenario[] = [
  {
    name: 'spa-filter-cycle',
    url: `file://${fixturesDir}/spa-like.html`,
    description:
      'Cycle through feed filter buttons (real card filtering + active-button toggle, no navigation) — small, realistic per-step DOM change',
    stepSelectors: [
      '.filter-btn[data-filter="articles"]',
      '.filter-btn[data-filter="discussions"]',
      '.filter-btn[data-filter="videos"]',
      '.filter-btn[data-filter="links"]',
      '.filter-btn[data-filter="all"]',
    ],
  },
  {
    name: 'form-shipping-cycle',
    url: `file://${fixturesDir}/form.html`,
    description:
      'Cycle through shipping-method radio buttons (checked-attribute toggle) — a second, independent per-step change shape so the delta-cost claim is not based on a single favorable scenario',
    stepSelectors: [
      'input[name="shipping_method"][value="express"]',
      'input[name="shipping_method"][value="overnight"]',
      'input[name="shipping_method"][value="standard"]',
    ],
  },
];
