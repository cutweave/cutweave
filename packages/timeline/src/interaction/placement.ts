// SPDX-License-Identifier: GPL-3.0-or-later
// Where a dragged clip or a dropped card lands (timeline spec §3.6, INV-40/41, D8/D9). The engine
// places exactly what it is asked; the CapCut feel (rounding a drop on the main track to the
// nearest cut) and the choice of placement mode live here, in the editor.
import type { TimelineItem, TimelineModel } from "../model.ts";
import { endOf, isLocked, itemsOn, trackOf } from "./doc.ts";
import type { DroppedMedia } from "./types.ts";

/**
 * The cuts of a main track once `lifted` items have left it (the layout the person sees while
 * dragging, D9): 0, then the end of each remaining item packed from 0.
 */
export function liftedCuts(
  model: TimelineModel,
  trackId: string,
  lifted: ReadonlySet<string> | null,
): readonly number[] {
  const cuts = [0];
  let t = 0;
  for (const it of itemsOn(model, trackId)) {
    if (lifted?.has(it.id) === true) continue;
    t += it.duration;
    cuts.push(t);
  }
  return cuts;
}

/** The cut nearest to `t` (the earlier one on a tie). */
export function nearestCut(cuts: readonly number[], t: number): number {
  let best = cuts[0] ?? 0;
  for (const c of cuts) if (Math.abs(c - t) < Math.abs(best - t)) best = c;
  return best;
}

/** Whether `[start, end)` meets an item of the track other than the `ignore`d ones. */
export function overlaps(
  model: TimelineModel,
  trackId: string,
  start: number,
  end: number,
  ignore: ReadonlySet<string> | null,
): boolean {
  for (const it of itemsOn(model, trackId)) {
    if (ignore?.has(it.id) === true) continue;
    if (it.start < end && endOf(it) > start) return true;
  }
  return false;
}

/** The item of a track that contains time `t`. */
export function itemAt(model: TimelineModel, trackId: string, t: number): TimelineItem | undefined {
  return itemsOn(model, trackId).find((it) => it.start <= t && t < endOf(it));
}

/** The clip that ends exactly where `item` starts (`start`) or starts where it ends (`end`). */
export function neighbour(
  model: TimelineModel,
  item: TimelineItem,
  edge: "start" | "end",
): TimelineItem | undefined {
  return itemsOn(model, item.trackId).find(
    (it) =>
      it.id !== item.id && (edge === "start" ? endOf(it) === item.start : it.start === endOf(item)),
  );
}

/** The track kind a card goes on: pictures on a video track, sound on an audio track. */
const kindFor = (media: DroppedMedia): "media" | "audio" =>
  media.kind === "audio" ? "audio" : "media";

/** The track under the pointer when the card fits it, else the first unlocked one that does. */
export function dropTrack(
  model: TimelineModel,
  hovered: string | null,
  media: DroppedMedia,
): string | null {
  const want = kindFor(media);
  const under = trackOf(model, hovered);
  if (under !== undefined && under.kind === want && !isLocked(model, under.id)) return under.id;
  return model.tracks.find((t) => t.kind === want && t.locked !== true)?.id ?? null;
}
