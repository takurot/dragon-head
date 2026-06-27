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
