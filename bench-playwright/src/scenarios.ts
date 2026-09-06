import { resolve, dirname } from 'path';
import { fileURLToPath } from 'url';

const fixturesDir = resolve(dirname(fileURLToPath(import.meta.url)), '..', 'fixtures');

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
    url: 'https://example.com',
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
