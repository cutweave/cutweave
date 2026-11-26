// SPDX-License-Identifier: GPL-3.0-or-later
// Adaptive ruler ticks. Pure: given the zoom, picks the major tick step (a "nice" duration in
// flicks, never finer than a frame) and how many minor ticks subdivide it.
import { FLICKS_PER_SECOND, type FrameRate, frameDurationFlicks, nominalFps } from "./time.ts";

export interface TickPlan {
  /** Major tick spacing, flicks. */
  readonly step: number;
  /** Minor ticks per major step (1 = none). */
  readonly divisions: number;
}

const SECOND_STEPS = [1, 2, 5, 10, 15, 30];
const MINUTE_STEPS = [1, 2, 5, 10, 15, 30];
const HOUR_STEPS = [1, 2, 4, 8, 12, 24];
const FRAME_STEPS = [1, 2, 5, 10];

/** Minor tick spacing never goes below this many CSS pixels. */
export const MIN_MINOR_PX = 8;

/** Candidate major steps in ascending order, flicks. */
export function tickCandidates(fps: FrameRate): number[] {
  const frame = frameDurationFlicks(fps);
  const out: number[] = [];
  const perSecond = nominalFps(fps);
  for (const f of FRAME_STEPS) if (f < perSecond) out.push(f * frame);
  for (const s of SECOND_STEPS) out.push(s * FLICKS_PER_SECOND);
  for (const m of MINUTE_STEPS) out.push(m * 60 * FLICKS_PER_SECOND);
  for (const h of HOUR_STEPS) out.push(h * 3600 * FLICKS_PER_SECOND);
  return out;
}

/** Time-based steps subdivide by the first of these that keeps minor ticks apart. */
const TIME_DIVISIONS = [10, 5, 4, 2];

/** The candidates of the last frame rate asked for (the ruler asks every frame). */
let lastFps: FrameRate | null = null;
let lastCandidates: number[] = [];

function candidatesFor(fps: FrameRate): number[] {
  if (lastFps === null || lastFps.num !== fps.num || lastFps.den !== fps.den) {
    lastFps = { num: fps.num, den: fps.den };
    lastCandidates = tickCandidates(fps);
  }
  return lastCandidates;
}

/**
 * Smallest candidate step whose spacing is at least `minLabelPx` (room for one timecode label),
 * plus the largest subdivision that keeps minor ticks at least `MIN_MINOR_PX` apart. Written into
 * `out` when given (the draw loop reuses one plan), else into a new object.
 */
export function chooseTickPlan(
  pxPerSec: number,
  fps: FrameRate,
  minLabelPx: number,
  out: { step: number; divisions: number } = { step: 1, divisions: 1 },
): TickPlan {
  const pxPerFlick = pxPerSec / FLICKS_PER_SECOND;
  const candidates = candidatesFor(fps);
  let step = candidates[candidates.length - 1] ?? 1;
  for (let i = 0; i < candidates.length; i++) {
    const c = candidates[i] as number;
    if (c * pxPerFlick >= minLabelPx) {
      step = c;
      break;
    }
  }
  const stepPx = step * pxPerFlick;
  // Frame-based steps subdivide per frame; time-based steps by 10, 5, 4 or 2.
  let divisions = 1;
  if (step < FLICKS_PER_SECOND) {
    const n = Math.round(step / frameDurationFlicks(fps));
    if (n > 1 && stepPx / n >= MIN_MINOR_PX) divisions = n;
  } else {
    for (let i = 0; i < TIME_DIVISIONS.length; i++) {
      const n = TIME_DIVISIONS[i] as number;
      if (stepPx / n >= MIN_MINOR_PX) {
        divisions = n;
        break;
      }
    }
  }
  out.step = step;
  out.divisions = divisions;
  return out;
}
