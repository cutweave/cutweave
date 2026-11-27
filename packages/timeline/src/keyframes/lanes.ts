// SPDX-License-Identifier: GPL-3.0-or-later
// Keyframe lanes (WP-2C.05): the rows under a clip, one per animated property, with a mark per
// keyframe. The host projects them from the document for the clips whose lanes are open; the
// layout makes room for them in the clip's track, the renderer draws them and hit-testing finds
// the key under the pointer, all from the geometry here so they cannot drift.
import type { Theme } from "../theme.ts";

/** How the segment that starts at a key moves to the next one: it picks the key's mark. */
export const KEY_GLYPHS = ["linear", "hold", "bezier", "spring"] as const;
export type KeyGlyph = (typeof KEY_GLYPHS)[number];

/** One animated property of an item. Times are absolute sequence flicks, ascending. */
export interface KeyframeLane {
  /** The engine's property ID (`opacity`, `transform.position`, ...). */
  readonly prop: string;
  /** The property's name as shown (already translated). */
  readonly label: string;
  /** Key times, sequence flicks, ascending, without duplicates. */
  readonly times: ArrayLike<number>;
  /** Per key: the index of its {@link KeyGlyph} in {@link KEY_GLYPHS}. */
  readonly glyphs: ArrayLike<number>;
  /** Indices of the selected keys, ascending (absent: none). */
  readonly selected?: ArrayLike<number>;
}

/** The open lanes of one item, top to bottom. */
export interface ItemLanes {
  readonly itemId: string;
  readonly lanes: readonly KeyframeLane[];
}

export const NO_LANES: readonly ItemLanes[] = [];

/** Keys closer than this (CSS px) collapse into one mark (level of detail). */
export const KEY_COLLAPSE_PX = 3;

/** Height of one lane row, CSS px (the `control-sm` token, so it follows the density). */
export function laneRowHeight(theme: Pick<Theme, "controlSm">): number {
  return theme.controlSm;
}

/** Half size of a key mark and how far from its center a pointer still hits it. */
export function keyRadius(theme: Pick<Theme, "space1">): number {
  return theme.space1;
}
export function keyReach(theme: Pick<Theme, "space1">): number {
  return theme.space1 + 2;
}

/** First index in ascending `arr` at or after `from` with a value >= `t`. */
export function lowerBound(arr: ArrayLike<number>, t: number, from = 0): number {
  let lo = from;
  let hi = arr.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if ((arr[mid] as number) < t) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** Whether ascending `selected` holds an index in `[from, to)`. */
export function anySelected(
  selected: ArrayLike<number> | undefined,
  from: number,
  to: number,
): boolean {
  if (selected === undefined || selected.length === 0) return false;
  const i = lowerBound(selected, from);
  return i < selected.length && (selected[i] as number) < to;
}

/**
 * The key of `lane` nearest to time `t` and within `reachFlicks` of it, or -1. The nearest of
 * the two neighbours, so a collapsed run of keys still answers with a real key.
 */
export function nearestKey(lane: KeyframeLane, t: number, reachFlicks: number): number {
  const j = lowerBound(lane.times, t);
  let best = -1;
  let bestD = reachFlicks;
  for (let c = j - 1; c <= j; c++) {
    if (c < 0 || c >= lane.times.length) continue;
    const d = Math.abs((lane.times[c] as number) - t);
    if (d <= bestD) {
      bestD = d;
      best = c;
    }
  }
  return best;
}
