import { describe, it, expect } from 'vitest';
import { argValue } from './cli-args.js';

describe('argValue', () => {
  it('extracts a plain value', () => {
    expect(argValue(['--output=report.md'], '--output')).toBe('report.md');
  });

  it('does not truncate a value containing "=" (ISSUE-278)', () => {
    expect(argValue(['--output=./results/a=b.json'], '--output')).toBe('./results/a=b.json');
  });

  it('returns undefined when the flag is absent', () => {
    expect(argValue(['--other=x'], '--output')).toBeUndefined();
  });

  it('does not match a flag name that is a prefix of another flag', () => {
    // "--output" must not match "--output-md=..."
    expect(argValue(['--output-md=report.md'], '--output')).toBeUndefined();
  });

  it('returns an empty string for a flag with an empty value', () => {
    expect(argValue(['--output='], '--output')).toBe('');
  });
});
