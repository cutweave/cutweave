// SPDX-License-Identifier: GPL-3.0-or-later
// Coordinate transforms between timeline time (flicks) and canvas CSS pixels. Pure functions over
// a plain `Viewport`, so they are testable without a canvas and allocation-free in the hot loop.
//
// Integer safety: times are integer flicks below 2^53. Pixel positions are doubles computed as
// `(t - scrollX) * (pxPerSec / FLICKS_PER_SECOND)`; the inverse rounds to the nearest flick, so
// `xToTime(timeToX(t)) === t` exactly for every integer `t` below ~2^46 flicks (about 27 hours)
// at any zoom, because the double error (about t * 2^-52 flicks) stays far below 0.5.
import { FLICKS_PER_SECOND } from "./time.ts";

export interface Viewport {
  /** Zoom, CSS pixels per second. */
  pxPerSec: number;
  /** Time (flicks) at the left edge of the lane area (x = headerWidth). */
  scrollX: number;
  /** Vertical scroll of the track area, CSS pixels. */
  scrollY: number;
  /** Canvas size, CSS pixels. */
  width: number;
  height: number;
  /** Width of the track-header column, CSS pixels. */
  headerWidth: number;
  /** Height of the ruler, CSS pixels. */
  rulerHeight: number;
}

/**
 * Zoom limits (WP-2A.09): from 0.01 px/s, where 4 hours fit in 144 px of lane (so in any lane),
 * to 20,000 px/s, where one frame is at least 100 px wide up to 200 fps (833 px at 24 fps).
 */
export const MIN_PX_PER_SEC = 0.01;
export const MAX_PX_PER_SEC = 20_000;

export function clampZoom(pxPerSec: number): number {
  return Math.min(MAX_PX_PER_SEC, Math.max(MIN_PX_PER_SEC, pxPerSec));
}

/** Flicks covered by one CSS pixel. */
export function flicksPerPx(vp: Pick<Viewport, "pxPerSec">): number {
  return FLICKS_PER_SECOND / vp.pxPerSec;
}

/** Canvas x (CSS px) of timeline time `t`. */
export function timeToX(vp: Viewport, t: number): number {
  return vp.headerWidth + (t - vp.scrollX) * (vp.pxPerSec / FLICKS_PER_SECOND);
}

/** Timeline time (integer flicks) at canvas x. */
export function xToTime(vp: Viewport, x: number): number {
  return Math.round(vp.scrollX + (x - vp.headerWidth) / (vp.pxPerSec / FLICKS_PER_SECOND));
}

/** Width in CSS px of a duration in flicks. */
export function durationToPx(vp: Pick<Viewport, "pxPerSec">, duration: number): number {
  return duration * (vp.pxPerSec / FLICKS_PER_SECOND);
}

/** Visible time range of the lane area, flicks (inclusive, fractional ends rounded outward). */
export function visibleTimeRange(vp: Viewport): { t0: number; t1: number } {
  return { t0: xToTime(vp, vp.headerWidth), t1: xToTime(vp, vp.width) };
}

/**
 * `{ pxPerSec, scrollX }` that shows `[t0, t1]` (flicks) across the lane with `marginPx` on each
 * side, within the zoom limits (a range too long for the lane starts at its left edge).
 */
export function fitRange(
  vp: Viewport,
  t0: number,
  t1: number,
  marginPx: number,
): { pxPerSec: number; scrollX: number } {
  const lane = Math.max(1, vp.width - vp.headerWidth - 2 * marginPx);
  const span = Math.max(1, t1 - t0);
  const pxPerSec = clampZoom((lane * FLICKS_PER_SECOND) / span);
  const scrollX = Math.round(t0 - (marginPx * FLICKS_PER_SECOND) / pxPerSec);
  return { pxPerSec, scrollX };
}

/** New `{ pxPerSec, scrollX }` after zooming by `factor` keeping the time under `anchorX` fixed. */
export function zoomAround(
  vp: Viewport,
  factor: number,
  anchorX: number,
): { pxPerSec: number; scrollX: number } {
  const pxPerSec = clampZoom(vp.pxPerSec * factor);
  const anchorTime = vp.scrollX + (anchorX - vp.headerWidth) / (vp.pxPerSec / FLICKS_PER_SECOND);
  const scrollX = anchorTime - (anchorX - vp.headerWidth) / (pxPerSec / FLICKS_PER_SECOND);
  return { pxPerSec, scrollX: Math.round(scrollX) };
}

/**
 * Clamp scroll to the content: horizontally to `[0, contentEnd]` (the last item's end can scroll
 * to the left edge), vertically to `[0, contentHeight - viewport track area]`.
 */
export function clampScroll(
  vp: Viewport,
  contentEnd: number,
  contentHeight: number,
): { scrollX: number; scrollY: number } {
  const trackArea = Math.max(0, vp.height - vp.rulerHeight);
  return {
    scrollX: Math.min(Math.max(0, Math.round(vp.scrollX)), Math.max(0, contentEnd)),
    scrollY: Math.min(Math.max(0, vp.scrollY), Math.max(0, contentHeight - trackArea)),
  };
}
