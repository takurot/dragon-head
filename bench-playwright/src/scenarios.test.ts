import { describe, it, expect } from 'vitest';
import { resolveExternalBaselineUrl } from './scenarios.js';

describe('resolveExternalBaselineUrl (ISSUE-293)', () => {
  it('returns the default for undefined', () => {
    expect(resolveExternalBaselineUrl(undefined)).toBe('https://example.com');
  });

  it('returns the default for an empty string', () => {
    expect(resolveExternalBaselineUrl('')).toBe('https://example.com');
  });

  it('returns the default for a whitespace-only string', () => {
    expect(resolveExternalBaselineUrl('   ')).toBe('https://example.com');
  });

  it('returns the default for a malformed URL', () => {
    expect(resolveExternalBaselineUrl('not a url')).toBe('https://example.com');
  });

  it('accepts a valid override', () => {
    expect(resolveExternalBaselineUrl('https://example.org')).toBe('https://example.org/');
  });

  it('trims surrounding whitespace on a valid override', () => {
    expect(resolveExternalBaselineUrl('  https://example.org  ')).toBe('https://example.org/');
  });
});
