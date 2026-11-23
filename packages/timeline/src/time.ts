// SPDX-License-Identifier: GPL-3.0-or-later
// Time helpers for the timeline. Time is flicks (arch §4): `number` on the TS side, always an
// integer below 2^53. These mirror `cutweave-model` (WP-1A.01) for the few operations the view
// needs. WP-2C.06 ships that crate as wasm in `packages/protocol`; swap this module's bodies for
// the wasm calls then. Nothing else in this package does frame or timecode math.

/** Flicks per second (arch §4). */
export const FLICKS_PER_SECOND = 705_600_000;

/** A rational frame rate, e.g. `{ num: 24000, den: 1001 }` for 23.976. */
export interface FrameRate {
  readonly num: number;
  readonly den: number;
}

/** Duration of one frame in flicks. Integer for every standard rate (23.976 ... 120). */
export function frameDurationFlicks(fps: FrameRate): number {
  return Math.round((FLICKS_PER_SECOND * fps.den) / fps.num);
}

/** Frame index containing time `t` (floor), exact for integer flicks. */
export function flicksToFrame(t: number, fps: FrameRate): number {
  const d = frameDurationFlicks(fps);
  let f = Math.floor(t / d);
  // Float division can land one off at exact multiples for huge values; correct with integers.
  if (f * d > t) f--;
  else if ((f + 1) * d <= t) f++;
  return f;
}

/** Start time of frame `frame` in flicks. */
export function frameToFlicks(frame: number, fps: FrameRate): number {
  return frame * frameDurationFlicks(fps);
}

/**
 * Snap `t` to the frame grid: `nearest` rounds half up, as the engine does; `floor` and `ceil`
 * give the boundary at or before / at or after `t` (limits that must stay inside a range).
 */
export function snapToFrame(
  t: number,
  fps: FrameRate,
  rounding: "floor" | "ceil" | "nearest" = "nearest",
): number {
  const d = frameDurationFlicks(fps);
  const f = flicksToFrame(t, fps);
  if (rounding === "floor") return f * d;
  if (rounding === "ceil") return f * d === t ? t : (f + 1) * d;
  return t - f * d >= d / 2 ? (f + 1) * d : f * d;
}

/** Whole frames per second used as the timecode base (24 for 23.976, 30 for 29.97). */
export function nominalFps(fps: FrameRate): number {
  return Math.round(fps.num / fps.den);
}

/** Drop-frame applies only to 29.97 and 59.94 (frames dropped per minute: 2 and 4). */
export function isDropFrameRate(fps: FrameRate): boolean {
  return fps.den === 1001 && (fps.num === 30000 || fps.num === 60000);
}

const pad2 = (n: number): string => (n < 10 ? `0${n}` : String(n));

/**
 * `HH:MM:SS:FF` timecode of the frame containing `t` (drop-frame renders `HH:MM:SS;FF` when
 * `dropFrame` is set and the rate supports it). Negative times clamp to zero.
 */
export function formatTimecode(t: number, fps: FrameRate, dropFrame = false): string {
  let frames = Math.max(0, flicksToFrame(t, fps));
  const base = nominalFps(fps);
  const drop = dropFrame && isDropFrameRate(fps);
  if (drop) {
    const dropPerMin = base === 30 ? 2 : 4;
    const per10Min = base * 600 - dropPerMin * 9;
    const perMin = base * 60 - dropPerMin;
    const tens = Math.floor(frames / per10Min);
    const rem = frames % per10Min;
    frames +=
      dropPerMin * 9 * tens +
      (rem > dropPerMin ? dropPerMin * Math.floor((rem - dropPerMin) / perMin) : 0);
  }
  const ff = frames % base;
  const totalSec = Math.floor(frames / base);
  const ss = totalSec % 60;
  const mm = Math.floor(totalSec / 60) % 60;
  const hh = Math.floor(totalSec / 3600);
  return `${pad2(hh)}:${pad2(mm)}:${pad2(ss)}${drop ? ";" : ":"}${pad2(ff)}`;
}
