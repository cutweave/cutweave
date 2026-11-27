// SPDX-License-Identifier: GPL-3.0-or-later
// Finding media in the bin (WP-1M.12): the filter and sort of the engine's `media.list`, applied to
// the editor's document mirror so a keystroke needs no round trip. The engine's Rust predicate
// (`crates/cutweave-ops/src/media/find.rs`) is the reference; `find.test.ts` holds this one to the
// shared fixture `crates/cutweave-ops/tests/fixtures/media-find.json`.
//
// Text: the name is the display name (ADR-0020), else the file name (`MediaCard.name`); terms split
// on space, tab, CR and LF; every term must occur in the name, a tag or the note, ignoring case and
// accents. Durations are flicks, bounds inclusive; stills have none and never match a bound. Tags
// compare whole, ignoring case and accents; `collection` keeps the members of a manual or smart
// collection (ADR-0036). Sorts are total (ties fall back to the ID) and `desc` is the exact reverse
// of `asc`.
import type { MediaFilter, MediaId, MediaSort, MediaSortBy } from "@cutweave/protocol";
import type { MediaCard } from "./cards.ts";
import { type FolderParents, folderScope } from "./folderScope.ts";

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
  image_sequence: 3,
  sequence_ref: 4,
};

interface Entry {
  readonly card: MediaCard;
  readonly name: string;
  /** The tags as matching sees them (folded). */
  readonly tags: readonly string[];
  /** The folded note, filled in on first use (`null`: no note). */
  note: string | null | undefined;
}

interface Index {
  readonly entries: readonly Entry[];
  /** Entries per sort, built on first use (ascending; `desc` reads them backwards). */
  readonly orders: Map<MediaSortBy, readonly Entry[]>;
}

const indexes = new WeakMap<readonly MediaCard[], Index>();
const NO_TAGS: readonly string[] = [];

function indexOf(cards: readonly MediaCard[]): Index {
  let index = indexes.get(cards);
  if (index === undefined) {
    index = {
      entries: cards.map((card) => ({
        card,
        name: foldText(card.name),
        tags: card.tags.length === 0 ? NO_TAGS : card.tags.map(foldText),
        note: card.note === null ? null : undefined,
      })),
      orders: new Map(),
    };
    indexes.set(cards, index);
  }
  return index;
}

/** The folded note of an entry (`null`: none). */
function noteOf(e: Entry): string | null {
  if (e.note === undefined) e.note = foldText(e.card.note ?? "");
  return e.note;
}

/** Whether `term` occurs in the name, a tag or the note. */
function hasTerm(e: Entry, term: string): boolean {
  if (e.name.includes(term)) return true;
  for (const tag of e.tags) if (tag.includes(term)) return true;
  const note = noteOf(e);
  return note?.includes(term) ?? false;
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
  return (
    filterCount(filter) === 0 &&
    termsOf(filter?.text).length === 0 &&
    (filter?.folder ?? null) === null &&
    (filter?.collection ?? null) === null &&
    filter?.recursive !== false
  );
}

/**
 * Number of active filters: kinds, a duration bound, use, problems, tags and a rating (the text is
 * the search field, not a filter; the folder and the collection are where the bin is).
 */
export function filterCount(filter: MediaFilter | null | undefined): number {
  if (filter === undefined || filter === null) return 0;
  return (
    ((filter.kinds?.length ?? 0) > 0 ? 1 : 0) +
    (filter.min_duration !== undefined && filter.min_duration !== null ? 1 : 0) +
    (filter.max_duration !== undefined && filter.max_duration !== null ? 1 : 0) +
    (filter.used !== undefined && filter.used !== null ? 1 : 0) +
    (filter.problems === true ? 1 : 0) +
    ((filter.tags?.length ?? 0) > 0 ? 1 : 0) +
    (filter.min_rating !== undefined && filter.min_rating !== null ? 1 : 0)
  );
}

/**
 * What the collection rules need of a collection (ADR-0036): its stored members or its filter. A
 * project's `collections` fits.
 */
export type CollectionKinds = Readonly<
  Record<
    string,
    | {
        readonly kind:
          | { readonly type: "manual"; readonly media?: readonly string[] }
          | { readonly type: "smart"; readonly filter: MediaFilter };
      }
    | undefined
  >
>;

type Pass = (e: Entry) => boolean;
const NOTHING: Pass = () => false;

/**
 * The predicate of `filter`, or `null` when it lets everything through. The engine's `Matcher`
 * (cutweave-ops, find.rs) in TypeScript. A collection that does not exist has no members here (the
 * engine answers `not_found`); a smart collection's own filter may not name a collection, and one
 * whose folder is gone has no members.
 */
function compile(
  filter: MediaFilter | null | undefined,
  usage: ReadonlyMap<string, number>,
  folders: FolderParents,
  collections: CollectionKinds,
  nested: boolean,
): Pass | null {
  const terms = termsOf(filter?.text);
  const kinds = filter?.kinds ?? [];
  const min = filter?.min_duration ?? null;
  const max = filter?.max_duration ?? null;
  const used = filter?.used ?? null;
  const problems = filter?.problems === true;
  const tags = (filter?.tags ?? []).map((t) => foldText(t.trim()));
  const minRating = filter?.min_rating ?? null;
  const folder = filter?.folder ?? null;
  // A smart collection scoped to a folder that no longer exists matches nothing.
  if (nested && folder !== null && folders[folder] === undefined) return NOTHING;
  // `folder` and `recursive`: the folders whose assets pass (`null` inside: the top level).
  const scope = folderScope(folders, filter);

  let members: ReadonlySet<string> | null = null;
  let smart: Pass | null = null;
  const collection = filter?.collection ?? null;
  if (collection !== null) {
    const kind = nested ? undefined : collections[collection]?.kind;
    if (kind === undefined) return NOTHING;
    if (kind.type === "manual") {
      members = new Set(kind.media ?? []);
    } else {
      if ((kind.filter.collection ?? null) !== null) return NOTHING;
      smart = compile(kind.filter, usage, folders, collections, true);
    }
  }

  const everything =
    scope === null &&
    members === null &&
    smart === null &&
    terms.length === 0 &&
    kinds.length === 0 &&
    min === null &&
    max === null &&
    used === null &&
    !problems &&
    tags.length === 0 &&
    minRating === null;
  if (everything) return null;

  return (e: Entry): boolean => {
    const card = e.card;
    if (scope !== null && !scope.has(card.folder)) return false;
    if (members !== null && !members.has(card.id)) return false;
    if (kinds.length > 0 && !kinds.includes(card.kind)) return false;
    if (min !== null || max !== null) {
      const d = card.duration;
      if (d === null) return false;
      if (min !== null && d < min) return false;
      if (max !== null && d > max) return false;
    }
    if (minRating !== null && (card.rating === null || card.rating < minRating)) return false;
    if (used !== null && usage.has(card.id) !== used) return false;
    if (problems && !card.broken) return false;
    for (const tag of tags) if (!e.tags.includes(tag)) return false;
    for (const term of terms) if (!hasTerm(e, term)) return false;
    return smart === null || smart(e);
  };
}

/**
 * The assets that pass `filter`, ordered by `sort`. `usage` says which assets sit on a timeline
 * (`selectMediaUsage`); `folders` and `collections` are the project's, read when the filter names a
 * folder or a collection. The result is cached per card list for sorting, so repeating a search
 * with a longer text only re-filters.
 */
export function findMedia(
  cards: readonly MediaCard[],
  usage: ReadonlyMap<string, number>,
  filter?: MediaFilter | null,
  sort?: MediaSort | null,
  folders?: FolderParents | null,
  collections?: CollectionKinds | null,
): MediaFound {
  const by = sort?.by ?? "imported";
  const desc = sort?.order === "desc";
  const index = indexOf(cards);
  const order = ascending(index, by);
  const pass = compile(filter, usage, folders ?? {}, collections ?? {}, false);

  const out: MediaCard[] = [];
  if (desc) {
    for (let i = order.length - 1; i >= 0; i--) {
      const e = order[i] as Entry;
      if (pass === null || pass(e)) out.push(e.card);
    }
  } else {
    for (const e of order) if (pass === null || pass(e)) out.push(e.card);
  }
  return { cards: out, total: cards.length };
}

/** The IDs of `findMedia`, for tests. */
export function findIds(
  cards: readonly MediaCard[],
  usage: ReadonlyMap<string, number>,
  filter?: MediaFilter | null,
  sort?: MediaSort | null,
  folders?: FolderParents | null,
  collections?: CollectionKinds | null,
): MediaId[] {
  return findMedia(cards, usage, filter, sort, folders, collections).cards.map((c) => c.id);
}
