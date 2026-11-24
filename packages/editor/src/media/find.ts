// SPDX-License-Identifier: GPL-3.0-or-later
// Finding media in the bin (WP-1M.12): the filter and sort of the engine's `media.list`, applied to
// the editor's document mirror so a keystroke needs no round trip. The engine's Rust predicate
// (`crates/cutweave-ops/src/media/find.rs`) is the reference; `find.test.ts` holds this one to the
// shared fixture `crates/cutweave-ops/tests/fixtures/media-find.json`.
//
// Text: the name is the file name (`mediaName`); terms split on space, tab, CR and LF; every term
// must occur, ignoring case and accents. Durations are flicks, bounds inclusive; stills have none
// and never match a bound. Sorts are total (ties fall back to the ID) and `desc` is the exact
// reverse of `asc`.
import type { MediaFilter, MediaId, MediaSort, MediaSortBy } from "@cutweave/protocol";
import type { MediaCard } from "./cards.ts";

export type { MediaFilter, MediaSort };

/** The order the bin opens with: newest import first. */
export const DEFAULT_SORT: Required<MediaSort> = { by: "imported", order: "desc" };

/** The text as matching sees it: accents removed, lower case. */
export function foldText(text: string): string {
  // ASCII needs no decomposition; most names are.
  // biome-ignore lint/suspicious/noControlCharactersInRegex: tests for plain ASCII
  const ascii = /^[\x00-\x7f]*$/.test(text);
  return (ascii ? text : text.normalize("NFD").replace(/\p{M}/gu, "")).toLowerCase();
}

/** The search terms of a text: split on space, tab, CR and LF, folded, empty parts dropped. */
export function termsOf(text: string | null | undefined): string[] {
  if (text === undefined || text === null) return [];
  return foldText(text)
    .split(/[ \t\r\n]+/)
    .filter((t) => t !== "");
}

/** Order of two strings by code point, which is what the engine's byte-wise `str::cmp` does. */
export function compareCodePoints(a: string, b: string): number {
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i++) {
    let x = a.charCodeAt(i);
    let y = b.charCodeAt(i);
    if (x !== y) {
      // UTF-16 puts surrogates (astral characters) below U+E000..U+FFFF; code points do not.
      if (x >= 0xd800 && y >= 0xd800) {
        x += x >= 0xe000 ? -0x800 : 0x2000;
        y += y >= 0xe000 ? -0x800 : 0x2000;
      }
      return x < y ? -1 : 1;
    }
  }
  return a.length === b.length ? 0 : a.length < b.length ? -1 : 1;
}

const KIND_RANK: Record<MediaCard["kind"], number> = {
  video: 0,
  audio: 1,
  image: 2,
  sequence_ref: 3,
};

interface Entry {
  readonly card: MediaCard;
  readonly name: string;
}

interface Index {
  readonly entries: readonly Entry[];
  /** Entries per sort, built on first use (ascending; `desc` reads them backwards). */
  readonly orders: Map<MediaSortBy, readonly Entry[]>;
}

const indexes = new WeakMap<readonly MediaCard[], Index>();

function indexOf(cards: readonly MediaCard[]): Index {
  let index = indexes.get(cards);
  if (index === undefined) {
    index = {
      entries: cards.map((card) => ({ card, name: foldText(card.name) })),
      orders: new Map(),
    };
    indexes.set(cards, index);
  }
  return index;
}

function compareEntries(by: MediaSortBy, a: Entry, b: Entry): number {
  let primary = 0;
  switch (by) {
    case "imported":
      break;
    case "name":
      primary = compareCodePoints(a.name, b.name);
      break;
    case "duration": {
      const x = a.card.duration ?? Number.NEGATIVE_INFINITY;
      const y = b.card.duration ?? Number.NEGATIVE_INFINITY;
      primary = x === y ? 0 : x < y ? -1 : 1;
      break;
    }
    case "kind":
      primary =
        KIND_RANK[a.card.kind] - KIND_RANK[b.card.kind] || compareCodePoints(a.name, b.name);
      break;
  }
  return primary !== 0 ? primary : compareCodePoints(a.card.id, b.card.id);
}

/** The entries in ascending order of `by`; the cards arrive in ID order already. */
function ascending(index: Index, by: MediaSortBy): readonly Entry[] {
  if (by === "imported") return index.entries;
  let order = index.orders.get(by);
  if (order === undefined) {
    order = [...index.entries].sort((a, b) => compareEntries(by, a, b));
    index.orders.set(by, order);
  }
  return order;
}

/** What the bin shows for a search. */
export interface MediaFound {
  /** The matching cards in the requested order. */
  readonly cards: readonly MediaCard[];
  /** How many assets there are in all. */
  readonly total: number;
}

/** Whether the filter lets everything through. */
export function isEmptyFilter(filter: MediaFilter | null | undefined): boolean {
  return filterCount(filter) === 0 && termsOf(filter?.text).length === 0;
}

/**
 * Number of active filters: kinds, a duration bound, use and problems (the text is the search
 * field, not a filter).
 */
export function filterCount(filter: MediaFilter | null | undefined): number {
  if (filter === undefined || filter === null) return 0;
  return (
    ((filter.kinds?.length ?? 0) > 0 ? 1 : 0) +
    (filter.min_duration !== undefined && filter.min_duration !== null ? 1 : 0) +
    (filter.max_duration !== undefined && filter.max_duration !== null ? 1 : 0) +
    (filter.used !== undefined && filter.used !== null ? 1 : 0) +
    (filter.problems === true ? 1 : 0)
  );
}

/**
 * The assets that pass `filter`, ordered by `sort`. `usage` says which assets sit on a timeline
 * (`selectMediaUsage`). The result is cached per card list for sorting, so repeating a search with
 * a longer text only re-filters.
 */
export function findMedia(
  cards: readonly MediaCard[],
  usage: ReadonlyMap<string, number>,
  filter?: MediaFilter | null,
  sort?: MediaSort | null,
): MediaFound {
  const by = sort?.by ?? "imported";
  const desc = sort?.order === "desc";
  const index = indexOf(cards);
  const order = ascending(index, by);

  const terms = termsOf(filter?.text);
  const kinds = filter?.kinds ?? [];
  const min = filter?.min_duration ?? null;
  const max = filter?.max_duration ?? null;
  const used = filter?.used ?? null;
  const problems = filter?.problems === true;
  const everything =
    terms.length === 0 &&
    kinds.length === 0 &&
    min === null &&
    max === null &&
    used === null &&
    !problems;

  const pass = (e: Entry): boolean => {
    const card = e.card;
    if (kinds.length > 0 && !kinds.includes(card.kind)) return false;
    if (min !== null || max !== null) {
      const d = card.duration;
      if (d === null) return false;
      if (min !== null && d < min) return false;
      if (max !== null && d > max) return false;
    }
    if (used !== null && usage.has(card.id) !== used) return false;
    if (problems && !card.broken) return false;
    for (const term of terms) if (!e.name.includes(term)) return false;
    return true;
  };

  const out: MediaCard[] = [];
  if (desc) {
    for (let i = order.length - 1; i >= 0; i--) {
      const e = order[i] as Entry;
      if (everything || pass(e)) out.push(e.card);
    }
  } else {
    for (const e of order) if (everything || pass(e)) out.push(e.card);
  }
  return { cards: out, total: cards.length };
}

/** The IDs of `findMedia`, for tests. */
export function findIds(
  cards: readonly MediaCard[],
  usage: ReadonlyMap<string, number>,
  filter?: MediaFilter | null,
  sort?: MediaSort | null,
): MediaId[] {
  return findMedia(cards, usage, filter, sort).cards.map((c) => c.id);
}
