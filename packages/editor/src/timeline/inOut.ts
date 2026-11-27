// SPDX-License-Identifier: GPL-3.0-or-later
// The arguments of the timeline's marker and in/out edits (WP-2A.10), shared by the keys (M, I,
// O), the toolbar and the menus so each says the same `timeline.add_marker` or
// `timeline.set_in_out` for the same moment. Times land on the sequence's frames.
import type { Project, SequenceId } from "@cutweave/protocol";
import { frameDurationFlicks, snapToFrame, type TimelineModel } from "@cutweave/timeline";
import { selectActiveSequenceId, selectInOut, selectTimelineModel } from "../state/selectors.ts";

/** The end of the last clip, flicks. */
export function contentEnd(model: TimelineModel): number {
  let end = 0;
  for (const i of model.items) end = Math.max(end, i.start + i.duration);
  return end;
}

/** `timeline.add_marker` at `at` (on its frame), or null without a sequence. */
export function markerArgs(
  project: Project | null,
  at: number,
): { readonly sequence: SequenceId; readonly t: number } | null {
  const sequence = selectActiveSequenceId(project);
  if (sequence === null) return null;
  return { sequence, t: snapToFrame(Math.max(0, at), selectTimelineModel(project).fps) };
}

export interface InOutArgs {
  readonly sequence: SequenceId;
  readonly start: number;
  readonly end: number;
}

/**
 * `timeline.set_in_out` that puts the in point (or the out point) at `at`: the other point stays
 * when it is still on the right side, else the out goes to the end of the clips (at least a frame
 * after the in) and the in to the start. Null without a sequence, or for an out point at 0.
 */
export function inOutArgs(
  project: Project | null,
  point: "in" | "out",
  at: number,
): InOutArgs | null {
  const sequence = selectActiveSequenceId(project);
  if (sequence === null) return null;
  const model = selectTimelineModel(project);
  const t = snapToFrame(Math.max(0, at), model.fps);
  const range = selectInOut(project);
  if (point === "in") {
    const frame = frameDurationFlicks(model.fps);
    const end =
      range !== null && range.out > t ? range.out : Math.max(contentEnd(model), t + frame);
    return { sequence, start: t, end };
  }
  if (t <= 0) return null;
  return { sequence, start: range !== null && range.in < t ? range.in : 0, end: t };
}
