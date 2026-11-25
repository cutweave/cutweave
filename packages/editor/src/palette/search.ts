// SPDX-License-Identifier: GPL-3.0-or-later
// Fuzzy search for the palette (WP-2H.04). Every word of the query must match somewhere in a
// row's text, as a subsequence (`spl hd` finds "Split at playhead"), ignoring case and accents.
// A match scores higher when it is contiguous, starts a word, or starts the text; ties keep the
// caller's order. A keystroke over 400 commands must stay well under a frame (16 ms), so the
// text is folded once when the list is built, not per keystroke.
import { fold } from "../settings/search.ts";

export interface Searchable {
  /** The row's text folded by {@link fold}: title, chips, summary and id, space-separated. */
  readonly haystack: string;
  /** Where the title ends in `haystack` (a match inside the title counts for more). */
  readonly titleEnd: number;
}

/** Builds the folded text a row is searched in. */
export function searchable(title: string, rest: readonly string[]): Searchable {
  const head = fold(title);
  return { haystack: [head, ...rest.map(fold)].join(" "), titleEnd: head.length };
}

const WORD_START = /[\s.:_\-/]/;

/** Score of `word` as a subsequence of `text`, or `null` when it is not one. */
function scoreWord(text: string, word: string, titleEnd: number): number | null {
  // A plain substring is the best case.
  const at = text.indexOf(word);
  if (at >= 0) {
    const boundary = at === 0 || WORD_START.test(text[at - 1] ?? " ");
    return 100 + (boundary ? 40 : 0) + (at === 0 ? 20 : 0) + (at < titleEnd ? 30 : 0);
  }
  let score = 0;
  let from = 0;
  let previous = -2;
  for (const ch of word) {
    const found = text.indexOf(ch, from);
    if (found < 0) return null;
    const boundary = found === 0 || WORD_START.test(text[found - 1] ?? " ");
    score += 1 + (found === previous + 1 ? 4 : 0) + (boundary ? 6 : 0) + (found < titleEnd ? 2 : 0);
    previous = found;
    from = found + 1;
  }
  return score;
}

/** The words of a query, folded; none when it is blank. */
export function words(query: string): string[] {
  return fold(query)
    .split(/\s+/)
    .filter((w) => w !== "");
}

/** The score of a row for the query words, or `null` when one word does not match. */
export function scoreRow(row: Searchable, queryWords: readonly string[]): number | null {
  let total = 0;
  for (const word of queryWords) {
    const s = scoreWord(row.haystack, word, row.titleEnd);
    if (s === null) return null;
    total += s;
  }
  return total;
}

/**
 * Indexes of the rows that match, best first; rows with equal scores keep their order. A blank
 * query matches every row in order.
 */
export function rank(rows: readonly Searchable[], query: string): number[] {
  const ws = words(query);
  if (ws.length === 0) return rows.map((_, i) => i);
  const scored: { i: number; s: number }[] = [];
  rows.forEach((row, i) => {
    const s = scoreRow(row, ws);
    if (s !== null) scored.push({ i, s });
  });
  scored.sort((a, b) => b.s - a.s || a.i - b.i);
  return scored.map((m) => m.i);
}
