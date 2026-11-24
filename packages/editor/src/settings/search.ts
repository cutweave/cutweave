// SPDX-License-Identifier: GPL-3.0-or-later
// Settings search (WP-1M.11): every word the person types must appear in a row's label,
// description or keywords, or in its page's title, in any order, ignoring case and accents. It
// runs over the registry in the UI language, and returns the rows grouped by page with the
// matching text marked. Settings search has no engine command: it filters the registry.

import type { TKey } from "../i18n/index.ts";
import type { SettingRowDef, SettingsPageDef } from "./registry.ts";

type Translate = (key: TKey) => string;

/** Lowercase, accents removed, one output character per input character (so offsets still line up). */
function foldChar(ch: string): string {
  const folded = ch.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
  return folded.length === ch.length ? folded : ch.toLowerCase();
}

export function fold(text: string): string {
  let out = "";
  for (const ch of text) out += foldChar(ch);
  return out;
}

/** The words of a query, folded; none when the query is blank. */
export function queryWords(query: string): string[] {
  return fold(query)
    .split(/\s+/)
    .filter((w) => w !== "");
}

export interface TextPart {
  text: string;
  match: boolean;
}

/** `text` cut into parts, the ranges that hold one of `words` marked. */
export function markMatches(text: string, words: readonly string[]): TextPart[] {
  const folded = fold(text);
  const marked = new Array<boolean>(text.length).fill(false);
  for (const word of words) {
    for (let at = folded.indexOf(word); at >= 0; at = folded.indexOf(word, at + word.length)) {
      for (let i = at; i < at + word.length && i < marked.length; i++) marked[i] = true;
    }
  }
  const parts: TextPart[] = [];
  for (let i = 0; i < text.length; i++) {
    const match = marked[i] === true;
    const last = parts[parts.length - 1];
    if (last !== undefined && last.match === match) last.text += text[i];
    else parts.push({ text: text[i] ?? "", match });
  }
  return parts;
}

export interface RowMatch {
  row: SettingRowDef;
  label: TextPart[];
  description: TextPart[];
}

export interface PageMatch {
  page: SettingsPageDef;
  rows: RowMatch[];
}

/** Whether every word is found in the row's words (label, description, keywords, page title). */
function matches(words: readonly string[], haystack: string): boolean {
  return words.every((word) => haystack.includes(word));
}

/** The pages with matching rows, in page order; empty for a blank query or no match. */
export function searchSettings(
  pages: readonly SettingsPageDef[],
  query: string,
  t: Translate,
): PageMatch[] {
  const words = queryWords(query);
  if (words.length === 0) return [];
  const found: PageMatch[] = [];
  for (const page of pages) {
    const title = t(page.title);
    const rows: RowMatch[] = [];
    for (const row of page.rows) {
      const label = t(row.label);
      const description = t(row.description);
      const keywords = [
        row.keywords === undefined ? "" : t(row.keywords),
        row.keywordsText?.() ?? "",
      ].join(" ");
      const haystack = fold(`${label}\n${description}\n${keywords}\n${title}`);
      if (!matches(words, haystack)) continue;
      rows.push({
        row,
        label: markMatches(label, words),
        description: markMatches(description, words),
      });
    }
    if (rows.length > 0) found.push({ page, rows });
  }
  return found;
}

export function countMatches(found: readonly PageMatch[]): number {
  return found.reduce((sum, page) => sum + page.rows.length, 0);
}
