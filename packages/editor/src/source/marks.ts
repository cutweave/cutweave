// SPDX-License-Identifier: GPL-3.0-or-later
// The Source view's in and out marks (WP-2A.14): what setting one does to the other, and the
// marks of the asset on screen. Marks are positions in the media's own time (flicks); the out
// mark is exclusive, like the timeline's. They live in the view state (`viewStore.sourceMarks`,
// the engine's `ui-state.json`), never in the document, so setting one is not an undo step.
import type { MediaId, SourceMarks } from "@cutweave/protocol";
import type { StoreApi } from "zustand";
import type { PlaybackStore } from "../state/playbackStore.ts";
import type { ViewStore } from "../state/viewStore.ts";

export type MarkPoint = "in" | "out";

const NONE: SourceMarks = { in: null, out: null };

/**
 * `marks` with the in (or out) mark at `at`, in a media `length` long. The other mark stays while
 * it is still on the right side, else it is dropped. An out mark at the very start, or an in mark
 * at the very end, would leave nothing to place: the marks are returned unchanged.
 */
export function withMark(
  marks: SourceMarks | undefined,
  point: MarkPoint,
  at: number,
  length: number,
): SourceMarks {
  const current = marks ?? NONE;
  const t = Math.min(length, Math.max(0, Math.round(at)));
  if (point === "in") {
    if (t >= length) return current;
    return { in: t, out: current.out != null && current.out > t ? current.out : null };
  }
  if (t <= 0) return current;
  return { in: current.in != null && current.in < t ? current.in : null, out: t };
}

/** The marked range of a media `length` long: the marks, or its ends where one is missing. */
export function markedRange(
  marks: SourceMarks | undefined,
  length: number,
): { readonly in: number; readonly out: number } | null {
  if (marks === undefined || (marks.in == null && marks.out == null)) return null;
  const start = Math.min(length, Math.max(0, marks.in ?? 0));
  const end = Math.min(length, Math.max(start, marks.out ?? length));
  return end > start ? { in: start, out: end } : null;
}

export interface MarkDeps {
  readonly view: StoreApi<ViewStore>;
  readonly playback: StoreApi<PlaybackStore>;
}

/** Sets the in or out mark of the source on screen at its playhead. `false` without a source. */
export function markSource(deps: MarkDeps, point: MarkPoint): boolean {
  const pb = deps.playback.getState();
  if (pb.source === null) return false;
  const media = pb.source.media as MediaId;
  const view = deps.view.getState();
  const next = withMark(view.sourceMarks[media], point, pb.sourcePlayhead, pb.source.duration);
  view.setSourceMarks(media, next);
  return true;
}

/** Clears both marks of the source on screen. */
export function clearSourceMarks(deps: MarkDeps): boolean {
  const source = deps.playback.getState().source;
  if (source === null) return false;
  deps.view.getState().setSourceMarks(source.media as MediaId, null);
  return true;
}
