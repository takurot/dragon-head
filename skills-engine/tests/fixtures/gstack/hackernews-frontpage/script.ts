/**
 * hackernews-frontpage — scrape the HN front page and emit JSON.
 *
 * Output protocol:
 *   stdout = a single JSON document on success: { stories: Story[], count }
 *   stderr = anything we want logged (currently nothing)
 *   exit 0 on success, nonzero on parse / network failure.
 *
 * The parser logic (`parseStoriesFromHtml`) is exported so script.test.ts can
 * exercise it against bundled HTML fixtures without spinning up the daemon.
 */

import { browse } from './_lib/browse-client';

export interface Story {
  /** 1-based rank as displayed on HN. */
  rank: number;
  /** HN item id (the integer in `tr.athing[id]`). */
  id: string;
  title: string;
  /** Outbound URL the title links to. */
  url: string;
  /** null when the row has no score (job postings). */
  points: number | null;
  /** null when the row has no comments link (job postings). */
  comments: number | null;
}

export interface Output {
  stories: Story[];
  count: number;
}

const FRONT_PAGE_URL = 'https://news.ycombinator.com/';

/**
 * Parse HN front-page HTML into Story[].
 *
 * HN's structure is stable: each story is a pair of rows.
 *   <tr class="athing submission" id="<itemid>">
 *     <td class="rank">N.</td>
 *     <td class="title">...</td>
 *     <td class="title"><span class="titleline"><a href="<url>">title</a> ...</span></td>
 *   </tr>
 *   <tr><td colspan="2"></td><td class="subtext"><span class="subline">
 *     <span class="score" id="score_<itemid>">N points</span>
 *     ... <a href="item?id=<itemid>">N comments</a>
 *   </span></td></tr>
 *
 * Job postings ("Foo (YC X25) is hiring...") omit the score and comments —
 * those fields come back as null.
 */
export function parseStoriesFromHtml(html: string): Story[] {
  const stories: Story[] = [];

  // Match every `<tr ...>` and check its attributes independently of order —
  // real HN markup always puts class before id, but relying on that order
  // makes the parser fragile to any upstream layout change.
  const rowRegex = /<tr\s+([^>]*)>([\s\S]*?)<\/tr>/g;

  let match: RegExpExecArray | null;
  let rank = 0;
  while ((match = rowRegex.exec(html)) !== null) {
    const attrs = match[1];
    // `\b` alone would also match "athing" inside a hyphenated class like
    // "not-athing" (a `-` is a word-boundary character). Require "athing" to
    // be its own whitespace-delimited class token instead.
    const classMatch = attrs.match(/\bclass="([^"]*)"/);
    if (!classMatch || !classMatch[1].split(/\s+/).includes('athing')) continue;
    const idMatch = attrs.match(/\bid="(\d+)"/);
    if (!idMatch) continue;
    const id = idMatch[1];
    const rowBody = match[2];

    // Title link: <span class="titleline"><a href="..." ...>title</a>
    const titleMatch = rowBody.match(/<span\s+class="titleline"[^>]*>\s*<a\s+href="([^"]+)"[^>]*>([\s\S]*?)<\/a>/);
    // Only count rows that actually parse as stories — otherwise a skipped
    // row (e.g. a malformed tr.athing) would leave a gap in the rank
    // sequence instead of the ranks matching displayed document order.
    if (!titleMatch) continue;
    rank++;
    const url = decodeHtmlEntities(titleMatch[1]);
    const title = stripTags(decodeHtmlEntities(titleMatch[2])).trim();

    // The next sibling tr should hold the subtext row. Bound the lookahead
    // to before the next story (tr.spacer marks the gap, then tr.athing),
    // or — for the last story on the page — before the "More" link row or
    // the closing </table>, whichever comes first. Bug if we don't bound:
    // the score from story N+1 (or the entire rest of the page, for the
    // last story) leaks into story N's subtext.
    const subtextStart = match.index + match[0].length;
    const tail = html.slice(subtextStart);
    const spacerIdx = tail.search(/<tr\b[^>]*\bclass="spacer\b/);
    const nextAthingIdx = tail.search(/<tr\b[^>]*\bclass="athing\b/);
    const moreIdx = tail.search(/<tr\b[^>]*\bclass="morespace\b/);
    const tableEndIdx = tail.search(/<\/table>/);
    const candidates = [spacerIdx, nextAthingIdx, moreIdx, tableEndIdx].filter(i => i >= 0);
    const boundary = candidates.length > 0 ? Math.min(...candidates) : tail.length;
    const subtextSlice = tail.slice(0, boundary);

    let points: number | null = null;
    let comments: number | null = null;

    const scoreMatch = subtextSlice.match(/<span\s+class="score"[^>]*>(\d+)\s*points?<\/span>/);
    if (scoreMatch) points = parseInt(scoreMatch[1], 10);

    // Comment count: an anchor like `<a href="item?id=...">N comments</a>`,
    // or `discuss` (treated as 0). Skip "hide" / "context" / "from" links.
    const commentsMatch = subtextSlice.match(/<a\s+href="item\?id=\d+"[^>]*>(\d+)\s*(?:&nbsp;)?\s*comments?<\/a>/);
    if (commentsMatch) {
      comments = parseInt(commentsMatch[1], 10);
    } else if (/discuss<\/a>/.test(subtextSlice)) {
      comments = 0;
    }

    stories.push({ rank, id, title, url, points, comments });
  }

  return stories;
}

function stripTags(s: string): string {
  return s.replace(/<[^>]*>/g, '');
}

// Named entities beyond the handful HN's own markup actually emits (kept for
// titles/URLs that happen to embed less common characters). `&amp;` must be
// decoded last so an entity's own literal `&` (e.g. an already-decoded
// `&amp;amp;` typo upstream) doesn't get re-interpreted as a fresh entity.
const NAMED_ENTITIES: Record<string, string> = {
  '&quot;': '"',
  '&#x27;': "'",
  '&#39;': "'",
  '&apos;': "'",
  '&lt;': '<',
  '&gt;': '>',
  '&nbsp;': ' ',
  '&mdash;': '—',
  '&ndash;': '–',
  '&hellip;': '…',
  '&copy;': '©',
  '&reg;': '®',
};

// Highest valid Unicode scalar value; String.fromCodePoint throws RangeError
// above this (and MDN doesn't special-case the surrogate-pair range either,
// so this single upper bound is the only guard actually needed here).
const MAX_CODE_POINT = 0x10ffff;

/** Decode one numeric entity's captured value, or leave malformed input untouched. */
function decodeNumericEntity(raw: string, radix: 10 | 16): string {
  const codePoint = parseInt(raw, radix);
  if (!Number.isFinite(codePoint) || codePoint < 0 || codePoint > MAX_CODE_POINT) {
    return radix === 16 ? `&#x${raw};` : `&#${raw};`;
  }
  return String.fromCodePoint(codePoint);
}

function decodeHtmlEntities(s: string): string {
  let out = s;
  for (const [entity, char] of Object.entries(NAMED_ENTITIES)) {
    out = out.split(entity).join(char);
  }
  // Numeric entities (decimal &#123; and hex &#x7B;), then the catch-all &amp;.
  // Malformed input (e.g. &#x110000; — beyond the Unicode scalar range) is
  // left as-is instead of throwing and aborting the whole scrape.
  out = out
    .replace(/&#x([0-9a-fA-F]+);/g, (_, hex) => decodeNumericEntity(hex, 16))
    .replace(/&#(\d+);/g, (_, dec) => decodeNumericEntity(dec, 10))
    .replace(/&amp;/g, '&');
  return out;
}

// ─── Main entry (only when run as a script, not when imported by tests) ─

if (import.meta.main) {
  await main();
}

async function main(): Promise<void> {
  try {
    await browse.goto(FRONT_PAGE_URL);
    const html = await browse.html();
    const stories = parseStoriesFromHtml(html);
    const output: Output = { stories, count: stories.length };
    process.stdout.write(JSON.stringify(output) + '\n');
  } catch (err: unknown) {
    // Without this catch, a rejected browse.goto()/html() (daemon down,
    // navigation timeout, ...) surfaces as an unhandled promise rejection —
    // no cleanup, and a stack trace on stdout instead of the documented
    // "stderr = anything we want logged, exit nonzero" protocol.
    const message = err instanceof Error ? err.message : String(err);
    process.stderr.write(`hackernews-frontpage: ${message}\n`);
    process.exitCode = 1;
  }
}
