// SPDX-License-Identifier: GPL-3.0-or-later
// Three-point placement (WP-2A.14, timeline spec §4.18): the pure function that turns the Source
// view's marks, the timeline's in and out and the playhead into the `at`, `source_in` and
// `duration` of a `timeline.insert` or `timeline.overwrite`. Every value is in one unit (flicks
// in the app, frames in the spec's fixtures); the source marks are positions in the media.

export interface ThreePointInput {
  /** The timeline's playhead. */
  readonly playhead: number;
  /** The timeline's in and out, or `null` when unset. */
  readonly timeline: { readonly in: number; readonly out: number } | null;
  /** The source's in mark, or `null`. */
  readonly sourceIn: number | null;
  /** The source's out mark (exclusive), or `null`. */
  readonly sourceOut: number | null;
  /** The media's length; `null` for a still. */
  readonly mediaLength: number | null;
  /** What a still without marks lasts (five seconds, INV-25). */
  readonly stillLength: number;
}

export type ThreePointResult =
  | {
      readonly ok: true;
      /** Where the clip starts on the timeline. */
      readonly at: number;
      /** The position in the media it starts from. */
      readonly sourceIn: number;
      readonly duration: number;
      /** The duration was cut to what is left of the media (INV-291). */
      readonly shortened: boolean;
    }
  | { readonly ok: false };

const INVALID: ThreePointResult = { ok: false };

/**
 * The placement of spec §4.18: the timeline in wins over the playhead; the source marks give the
 * length when there are any, else the timeline range, else the rest of the media. The length never
 * reads past the media (INV-291); a source out at or before the in, or a timeline range without a
 * length, is invalid (INV-292).
 */
export function threePoint(input: ThreePointInput): ThreePointResult {
  const { sourceIn: markIn, sourceOut: markOut, mediaLength: length, timeline } = input;
  if (markIn !== null && markOut !== null && markOut <= markIn) return INVALID;
  if (timeline !== null && timeline.out <= timeline.in) return INVALID;
  const sourceIn = markIn ?? 0;
  const toEnd = length === null ? input.stillLength : length - sourceIn;
  const at = timeline === null ? input.playhead : timeline.in;
  let duration: number;
  if (markIn !== null || markOut !== null) duration = markOut === null ? toEnd : markOut - sourceIn;
  else if (timeline !== null) duration = timeline.out - timeline.in;
  else duration = toEnd;
  let shortened = false;
  if (length !== null && sourceIn + duration > length) {
    duration = length - sourceIn;
    shortened = true;
  }
  if (duration < 1) return INVALID;
  return { ok: true, at, sourceIn, duration, shortened };
}
