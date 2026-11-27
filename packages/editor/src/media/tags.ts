// SPDX-License-Identifier: GPL-3.0-or-later
// Tags, ratings and notes in the bin (WP-2F.06b, ADR-0036): the rules the engine applies
// (`cutweave-model/src/collection.rs`: `tag_problem`, `tag_key`, `note_problem`), in TypeScript, so
// the tag editor can say what is wrong before a command is sent and can show which tags a
// selection has. The engine stays the authority: every change is one of its commands
// (`tagActions.ts`), and its own message is shown when it refuses.
//
// A tag is 1 to 64 characters after trimming, without a comma or control characters. Two tags are
// the same when they are equal ignoring case and accents (`foldText`, the folding search uses); an
// asset keeps the spelling it got first.
import type { MediaCard } from "./cards.ts";
import { compareCodePoints, foldText } from "./find.ts";

/** Longest tag, in characters. */
export const MAX_TAG_CHARS = 64;
/** Most tags on one asset. */
export const MAX_TAGS_PER_ASSET = 64;
/** Longest note, in characters. */
export const MAX_NOTE_CHARS = 2000;
/** From this many characters on, the note field shows its count. */
export const NOTE_COUNT_FROM = 1800;
/** Most assets one tag or rating command takes. */
export const MAX_TAG_MEDIA = 256;
/** Most tags one `media.add_tags`, `media.remove_tags` or filter takes. */
export const MAX_TAGS_PER_CALL = 16;
/** The ratings an asset can have (whole stars). */
export const RATINGS = [1, 2, 3, 4, 5] as const;
export type Rating = (typeof RATINGS)[number];

/** Why a text cannot be a tag. */
export type TagProblem = "empty" | "tooLong" | "comma" | "control";

/** Characters as the engine counts them: code points. */
export const charCount = (text: string): number => {
  let n = 0;
  for (const _ of text) n++;
  return n;
};

/** A typed tag as it is stored: without the space around it. */
export const normalizeTag = (text: string): string => text.trim();

/** The form tags are compared and sorted in: `Café` and `cafe` are the same tag. */
export const tagKey = (tag: string): string => foldText(tag);

/** Why the (trimmed) `tag` cannot be stored, or `null` when it can. */
export function tagProblem(tag: string): TagProblem | null {
  if (tag.trim() === "") return "empty";
  if (charCount(tag) > MAX_TAG_CHARS) return "tooLong";
  if (tag.includes(",")) return "comma";
  if (/\p{Cc}/u.test(tag)) return "control";
  return null;
}

/**
 * The tags in a typed or pasted text: split on commas and line breaks, trimmed, blanks dropped,
 * one per key (the first spelling stays). They are not checked: `tagProblem` does that.
 */
export function parseTagEntry(text: string): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const part of text.split(/[,\r\n]+/)) {
    const tag = normalizeTag(part);
    const key = tagKey(tag);
    if (tag === "" || seen.has(key)) continue;
    seen.add(key);
    out.push(tag);
  }
  return out;
}

export interface TagCount {
  readonly tag: string;
  /** How many assets have it. */
  readonly count: number;
}

/**
 * Every tag of the project with the number of assets that carry it: what the engine's
 * `media.list_tags` answers, read from the mirror (cards arrive in ID order, so the spelling is
 * the first asset's, and the list is sorted by key).
 */
export function projectTags(cards: readonly MediaCard[]): TagCount[] {
  const byKey = new Map<string, { tag: string; count: number }>();
  for (const card of cards) {
    for (const tag of card.tags) {
      const key = tagKey(tag);
      const entry = byKey.get(key);
      if (entry === undefined) byKey.set(key, { tag, count: 1 });
      else entry.count++;
    }
  }
  return [...byKey.entries()]
    .sort((a, b) => compareCodePoints(a[0], b[0]))
    .map(([, entry]) => entry);
}

export interface SelectionTag extends TagCount {
  /** Every selected asset has it; `false`: only some do. */
  readonly all: boolean;
}

/**
 * The tags of a selection: each with how many of the assets have it and whether all do, sorted by
 * key. One asset: every tag is `all`.
 */
export function selectionTags(cards: readonly MediaCard[]): SelectionTag[] {
  return projectTags(cards).map((entry) => ({ ...entry, all: entry.count === cards.length }));
}

/** The rating the selection shares: a number, `null` when none is rated, `"mixed"` otherwise. */
export function sharedRating(cards: readonly MediaCard[]): number | null | "mixed" {
  const first = cards[0];
  if (first === undefined) return null;
  return cards.every((c) => c.rating === first.rating) ? first.rating : "mixed";
}

/** Most suggestions the tag field lists. */
export const MAX_SUGGESTIONS = 8;

/**
 * The project's tags that fit what is typed, for the tag field: those that start with it first,
 * then those that contain it, each group in the list's own order. Tags in `have` (every selected
 * asset has them already) are left out. Nothing typed: the most used tags.
 */
export function suggestTags(
  all: readonly TagCount[],
  typed: string,
  have: readonly string[] = [],
  limit = MAX_SUGGESTIONS,
): string[] {
  const skip = new Set(have.map(tagKey));
  const open = all.filter((entry) => !skip.has(tagKey(entry.tag)));
  const needle = tagKey(normalizeTag(typed));
  if (needle === "") {
    return [...open]
      .sort((a, b) => b.count - a.count)
      .slice(0, limit)
      .map((entry) => entry.tag);
  }
  const starts: string[] = [];
  const contains: string[] = [];
  for (const entry of open) {
    const key = tagKey(entry.tag);
    if (key.startsWith(needle)) starts.push(entry.tag);
    else if (key.includes(needle)) contains.push(entry.tag);
  }
  return [...starts, ...contains].slice(0, limit);
}

/**
 * What committing the note field does: nothing when the text is the stored note (the space around
 * it does not count), else the note to send (`null`: remove it).
 */
export function notePlan(
  text: string,
  stored: string | null,
): { readonly change: false } | { readonly change: true; readonly note: string | null } {
  const next = text.trim();
  if (next === (stored ?? "")) return { change: false };
  return { change: true, note: next === "" ? null : next };
}

/** Whether the note is too long to store. */
export const noteTooLong = (text: string): boolean => charCount(text.trim()) > MAX_NOTE_CHARS;
