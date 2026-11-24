// SPDX-License-Identifier: GPL-3.0-or-later
// Presenter logic shared by the preview paths (WP-1M.04, arch §10 step 3).
//
// - `playheadAt`: the engine reports the playhead at most 30 times a second (`playback.state`);
//   a 60 Hz presenter extrapolates it from the last sample at the reported rate. At 1x that is
//   the audio clock (the engine's `t` is "the audio audible now").
// - `selectFrame`: of the frames on hand, the one due now: forward, the newest whose
//   `pts <= clock - latency`; in reverse, the newest whose `pts <= clock + latency` (the frame
//   containing that time, which is the one just reached when time runs down). Spike S1 found
//   that this clock-scheduled choice presents every frame on time where "newest wins" drops
//   them. The GPU path makes the same choice in the bridge addon (`frameAtClock`); the JPEG
//   fallback makes it here.
import { FLICKS_PER_SECOND } from "@cutweave/timeline";

/** One playhead reading: `t` (flicks) at page time `at` (ms, `performance.now()`). */
export interface PlayheadSample {
  readonly t: number;
  /** 1 = normal, negative = reverse, 0 = stopped. */
  readonly rate: number;
  readonly playing: boolean;
  readonly at: number;
}

/** The playhead at page time `now` (ms), extrapolated while playing; integer flicks, `>= 0`. */
export function playheadAt(sample: PlayheadSample, now: number): number {
  if (!sample.playing || sample.rate === 0) return sample.t;
  const t = sample.t + ((now - sample.at) * sample.rate * FLICKS_PER_SECOND) / 1000;
  return Math.max(0, Math.round(t));
}

export interface Timed {
  /** Presentation time, flicks. */
  readonly pts: number;
}

export interface SelectOptions {
  /** Playback rate; 0 when stopped. */
  readonly rate: number;
  /** Clock margin in flicks (how late a frame may be shown), default 0. */
  readonly latency?: number;
}

/**
 * The frame due at `clock` (flicks), or `null` when none is due yet (keep the current one).
 * Stopped, the frame containing the playhead (the newest with `pts <= clock`, with no latency:
 * the bridge's rule, so a pause mid-frame never shows the next frame), or the nearest one when
 * every frame lies after it.
 */
export function selectFrame<T extends Timed>(
  frames: readonly T[],
  clock: number,
  options: SelectOptions,
): T | null {
  if (options.rate === 0) {
    let containing: T | null = null;
    let after: T | null = null;
    for (const f of frames) {
      if (f.pts <= clock) {
        if (containing === null || f.pts > containing.pts) containing = f;
      } else if (after === null || f.pts < after.pts) {
        after = f;
      }
    }
    return containing ?? after;
  }
  const latency = options.latency ?? 0;
  const target = options.rate < 0 ? clock + latency : clock - latency;
  let best: T | null = null;
  for (const f of frames) {
    if (f.pts <= target && (best === null || f.pts > best.pts)) best = f;
  }
  return best;
}

/** Frames that can never be due again once `shown` is on screen, playing in `rate`'s direction. */
export function staleFrames<T extends Timed>(
  frames: readonly T[],
  shown: T | null,
  rate: number,
): T[] {
  if (shown === null) return [];
  return frames.filter((f) => f !== shown && (rate < 0 ? f.pts > shown.pts : f.pts < shown.pts));
}
