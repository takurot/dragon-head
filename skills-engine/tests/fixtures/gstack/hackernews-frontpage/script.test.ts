/**
 * hackernews-frontpage script tests — exercise parseStoriesFromHtml against
 * the bundled HN fixture. No daemon, no network: the parser is a pure function
 * over HTML, so we test it directly.
 */

import { describe, it, expect } from 'bun:test';
import * as fs from 'fs';
import * as path from 'path';
import { parseStoriesFromHtml } from './script';

const FIXTURE = fs.readFileSync(
  path.join(__dirname, 'fixtures', 'hn-2026-04-26.html'),
  'utf-8',
);

describe('parseStoriesFromHtml against bundled HN fixture', () => {
  it('returns 5 stories (matching the fixture)', () => {
    const stories = parseStoriesFromHtml(FIXTURE);
    expect(stories).toHaveLength(5);
  });

  it('assigns 1-based ranks in document order', () => {
    const stories = parseStoriesFromHtml(FIXTURE);
    expect(stories.map(s => s.rank)).toEqual([1, 2, 3, 4, 5]);
  });

  it('extracts ids matching the tr.athing[id] attribute', () => {
    const stories = parseStoriesFromHtml(FIXTURE);
    expect(stories.map(s => s.id)).toEqual([
      '40000001', '40000002', '40000003', '40000004', '40000005',
    ]);
  });

  it('extracts titles and decodes HTML entities', () => {
    const stories = parseStoriesFromHtml(FIXTURE);
    expect(stories[0].title).toBe('Show HN: A toy compiler in 200 lines');
    expect(stories[1].title).toBe('Database internals: writing an LSM tree');
    expect(stories[3].title).toBe("Ask HN: What's your most underrated tool?");
    expect(stories[4].title).toBe('Why quantum & chess engines disagree');
  });

  it('extracts URLs and decodes ampersands', () => {
    const stories = parseStoriesFromHtml(FIXTURE);
    expect(stories[0].url).toBe('https://example.com/blog-post-1');
    expect(stories[1].url).toBe('https://example.org/database-internals');
    expect(stories[4].url).toBe('https://example.io/quantum&chess');
  });

  it('parses point counts as numbers', () => {
    const stories = parseStoriesFromHtml(FIXTURE);
    expect(stories[0].points).toBe(412);
    expect(stories[1].points).toBe(298);
    expect(stories[3].points).toBe(156);
    expect(stories[4].points).toBe(73);
  });

  it('parses comment counts as numbers', () => {
    const stories = parseStoriesFromHtml(FIXTURE);
    expect(stories[0].comments).toBe(87);
    expect(stories[1].comments).toBe(152);
    expect(stories[4].comments).toBe(12);
  });

  it('treats "discuss" links as 0 comments', () => {
    const stories = parseStoriesFromHtml(FIXTURE);
    expect(stories[3].comments).toBe(0);
  });

  it('returns null points + null comments for job postings', () => {
    const stories = parseStoriesFromHtml(FIXTURE);
    // Story #3 is the YC-hiring row in the fixture.
    expect(stories[2].title).toContain('YC W26');
    expect(stories[2].points).toBeNull();
    expect(stories[2].comments).toBeNull();
  });

  it('returns [] for empty HTML', () => {
    expect(parseStoriesFromHtml('')).toEqual([]);
  });

  it('returns [] for HTML with no story rows', () => {
    expect(parseStoriesFromHtml('<html><body><p>nothing here</p></body></html>')).toEqual([]);
  });

  it('does not fabricate stories from arbitrary tr.athing rows missing titleline', () => {
    const html = '<tr class="athing" id="999"><td>nothing</td></tr>';
    expect(parseStoriesFromHtml(html)).toEqual([]);
  });
});

describe('decodeHtmlEntities coverage (ISSUE-291)', () => {
  it('decodes named entities beyond the original 7', () => {
    const html =
      '<tr class="athing" id="1"><td class="title"><span class="titleline">' +
      '<a href="https://example.com/x">Foo &mdash; Bar &hellip; &copy; 2026</a></span></td></tr>';
    const stories = parseStoriesFromHtml(html);
    expect(stories[0].title).toBe('Foo — Bar … © 2026');
  });

  it('decodes numeric decimal and hex entities', () => {
    const html =
      '<tr class="athing" id="1"><td class="title"><span class="titleline">' +
      '<a href="https://example.com/x">Caf&#233; &#x2014; na&#xef;ve</a></span></td></tr>';
    const stories = parseStoriesFromHtml(html);
    expect(stories[0].title).toBe('Café — naïve');
  });

  it('leaves an out-of-range numeric entity untouched instead of throwing', () => {
    const html =
      '<tr class="athing" id="1"><td class="title"><span class="titleline">' +
      '<a href="https://example.com/x">Bad&#x110000;Entity and&#9999999;Too</a></span></td></tr>';
    expect(() => parseStoriesFromHtml(html)).not.toThrow();
    const stories = parseStoriesFromHtml(html);
    expect(stories[0].title).toBe('Bad&#x110000;Entity and&#9999999;Too');
  });
});

describe('rank sequencing does not gap on skipped rows (ISSUE-291)', () => {
  it('assigns sequential ranks even when an earlier athing row has no titleline', () => {
    const html = [
      '<tr class="athing" id="1"><td>no titleline here</td></tr>',
      '<tr class="athing" id="2"><td class="title"><span class="titleline"><a href="https://example.com/a">A</a></span></td></tr>',
      '<tr class="athing" id="3"><td class="title"><span class="titleline"><a href="https://example.com/b">B</a></span></td></tr>',
    ].join('\n');
    const stories = parseStoriesFromHtml(html);
    expect(stories.map(s => s.rank)).toEqual([1, 2]);
    expect(stories.map(s => s.id)).toEqual(['2', '3']);
  });
});

describe('row matching is attribute-order independent (ISSUE-291)', () => {
  it('matches a tr.athing row with id before class', () => {
    const html =
      '<tr id="99" class="athing submission"><td class="title"><span class="titleline">' +
      '<a href="https://example.com/x">Reordered attrs</a></span></td></tr>';
    const stories = parseStoriesFromHtml(html);
    expect(stories).toHaveLength(1);
    expect(stories[0].id).toBe('99');
    expect(stories[0].title).toBe('Reordered attrs');
  });

  it('does not treat a hyphenated class like "not-athing" as an athing row', () => {
    const html =
      '<tr id="1" class="not-athing"><td class="title"><span class="titleline">' +
      '<a href="https://example.com/x">Should be ignored</a></span></td></tr>';
    expect(parseStoriesFromHtml(html)).toEqual([]);
  });
});

describe('last-story subtext is bounded, not the rest of the page (ISSUE-291)', () => {
  it('does not attribute a decoy score/comments count past </table> to a trailing job posting', () => {
    // A job posting's own subtext row has no score/comments (see the real
    // fixture's story #3). With no spacer/next-athing/</table> bound, the
    // old code would search the *entire rest of the page* for the next
    // <span class="score">/comments link and wrongly attribute an unrelated
    // page-footer number to this story.
    const html = [
      '<table class="itemlist">',
      '<tr class="athing" id="1"><td class="title"><span class="titleline">',
      '<a href="https://example.com/x">Acme is hiring</a></span></td></tr>',
      '<tr><td class="subtext"><span class="subline">just now</span></td></tr>',
      '</table>',
      '<p>footer text mentioning <span class="score">999 points</span> and',
      '<a href="item?id=999">500 comments</a> from an unrelated page section</p>',
    ].join('\n');
    const stories = parseStoriesFromHtml(html);
    expect(stories).toHaveLength(1);
    expect(stories[0].points).toBeNull();
    expect(stories[0].comments).toBeNull();
  });

  it('still parses the last story\'s own score/comments when present', () => {
    const html = [
      '<table class="itemlist">',
      '<tr class="athing" id="1"><td class="title"><span class="titleline">',
      '<a href="https://example.com/x">Only story</a></span></td></tr>',
      '<tr><td class="subtext"><span class="subline">',
      '<span class="score">10 points</span> <a href="item?id=1">2 comments</a>',
      '</span></td></tr>',
      '</table>',
      '<span class="score">999 points</span> <a href="item?id=999">500 comments</a>',
    ].join('\n');
    const stories = parseStoriesFromHtml(html);
    expect(stories[0].points).toBe(10);
    expect(stories[0].comments).toBe(2);
  });
});

describe('output shape', () => {
  it('every story has all required keys', () => {
    const stories = parseStoriesFromHtml(FIXTURE);
    for (const s of stories) {
      expect(typeof s.rank).toBe('number');
      expect(typeof s.id).toBe('string');
      expect(typeof s.title).toBe('string');
      expect(typeof s.url).toBe('string');
      // points/comments may be null for job rows
      expect(s.points === null || typeof s.points === 'number').toBe(true);
      expect(s.comments === null || typeof s.comments === 'number').toBe(true);
    }
  });
});
