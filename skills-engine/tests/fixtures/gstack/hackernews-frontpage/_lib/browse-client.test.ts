/**
 * browse-client tests — cover the tabId parsing and state-file resolution
 * paths directly (ISSUE-291). No daemon, no network: these exercise
 * construction-time logic only, never `command()`.
 */

import { describe, it, expect, afterEach } from 'bun:test';
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import { BrowseClient, resolveBrowseAuth } from './browse-client';

const ORIGINAL_BROWSE_TAB = process.env.BROWSE_TAB;

afterEach(() => {
  if (ORIGINAL_BROWSE_TAB === undefined) {
    delete process.env.BROWSE_TAB;
  } else {
    process.env.BROWSE_TAB = ORIGINAL_BROWSE_TAB;
  }
});

describe('BrowseClient tabId parsing', () => {
  it('parses a numeric BROWSE_TAB', () => {
    process.env.BROWSE_TAB = '3';
    const client = new BrowseClient({ port: 1, token: 't' });
    expect(client.tabId).toBe(3);
  });

  it('falls back to undefined (not NaN) for a non-numeric BROWSE_TAB', () => {
    process.env.BROWSE_TAB = 'not-a-number';
    const client = new BrowseClient({ port: 1, token: 't' });
    expect(client.tabId).toBeUndefined();
    expect(Number.isNaN(client.tabId)).toBe(false);
  });

  it('leaves tabId undefined when BROWSE_TAB is unset', () => {
    delete process.env.BROWSE_TAB;
    const client = new BrowseClient({ port: 1, token: 't' });
    expect(client.tabId).toBeUndefined();
  });

  it('an explicit opts.tabId wins over BROWSE_TAB', () => {
    process.env.BROWSE_TAB = '3';
    const client = new BrowseClient({ port: 1, token: 't', tabId: 7 });
    expect(client.tabId).toBe(7);
  });
});

describe('resolveBrowseAuth state-file fallback', () => {
  it('does not throw and falls through past a corrupt state file', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'browse-client-test-'));
    const stateFile = path.join(dir, 'browse.json');
    fs.writeFileSync(stateFile, '{ not valid json');

    // No env vars set and the state file is corrupt, so this must still
    // reach the documented "cannot find daemon" error — not throw a raw
    // JSON.parse SyntaxError, and not hang.
    expect(() => resolveBrowseAuth({ stateFile })).toThrow(/cannot find daemon/);

    fs.rmSync(dir, { recursive: true, force: true });
  });

  it('reads a well-formed state file', () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'browse-client-test-'));
    const stateFile = path.join(dir, 'browse.json');
    fs.writeFileSync(stateFile, JSON.stringify({ port: 4321, token: 'abc' }));

    const auth = resolveBrowseAuth({ stateFile });
    expect(auth).toEqual({ port: 4321, token: 'abc', source: 'state-file' });

    fs.rmSync(dir, { recursive: true, force: true });
  });
});
