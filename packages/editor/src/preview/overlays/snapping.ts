// SPDX-License-Identifier: GPL-3.0-or-later
// Snapping for the preview's handles (WP-2H.06): while a clip moves, scales or is cropped, its
// edges and centre catch the canvas edges, centre, halves and quarters (so two clips fill the
// halves of a frame) and the edges and centres of the other clips on screen. Everything is in
// canvas pixels; the catch distance is given in canvas pixels too (the caller divides a screen
// distance by the preview zoom, so it feels the same at 25 % and 100 %).
import type { Pt, Size } from "./geometry.ts";

/** A guide line to draw: a vertical one (`axis: "x"`) at canvas x, or a horizontal one. */
export interface Guide {
  readonly axis: "x" | "y";
  readonly at: number;
}

export interface SnapTargets {
  readonly xs: readonly number[];
  readonly ys: readonly number[];
}

/** Screen pixels within which an edge catches a target. */
export const SNAP_DISTANCE_PX = 6;

/** The canvas's own lines: its edges, quarters and centre (the halves are the centre). */
export function canvasTargets(canvas: Size): SnapTargets {
  return {
    xs: [0, 0.25, 0.5, 0.75, 1].map((f) => f * canvas.w),
    ys: [0, 0.25, 0.5, 0.75, 1].map((f) => f * canvas.h),
  };
}

/** The canvas's lines plus the edges and centres of other clips' bounds. */
export function targetsWith(
  canvas: Size,
  others: readonly (readonly [number, number, number, number])[],
): SnapTargets {
  const base = canvasTargets(canvas);
  return {
    xs: [...base.xs, ...others.flatMap((b) => [b[0], (b[0] + b[2]) / 2, b[2]])],
    ys: [...base.ys, ...others.flatMap((b) => [b[1], (b[1] + b[3]) / 2, b[3]])],
  };
}

/** The shift that brings one of `candidates` onto the nearest of `targets`, within `within`. */
function nearest(
  candidates: readonly number[],
  targets: readonly number[],
  within: number,
): { shift: number; at: number } | null {
  let best: { shift: number; at: number } | null = null;
  for (const c of candidates) {
    for (const t of targets) {
      const d = t - c;
      if (Math.abs(d) <= within && (best === null || Math.abs(d) < Math.abs(best.shift))) {
        best = { shift: d, at: t };
      }
    }
  }
  return best;
}

export interface MoveSnap {
  readonly dx: number;
  readonly dy: number;
  readonly guides: readonly Guide[];
}

/**
 * How far to shift a box (`bounds`: left, top, right, bottom) so its left, centre or right edge
 * lands on a vertical target and its top, centre or bottom on a horizontal one.
 */
export function snapBounds(
  bounds: readonly [number, number, number, number],
  targets: SnapTargets,
  within: number,
): MoveSnap {
  const [x0, y0, x1, y1] = bounds;
  const x = nearest([x0, (x0 + x1) / 2, x1], targets.xs, within);
  const y = nearest([y0, (y0 + y1) / 2, y1], targets.ys, within);
  const guides: Guide[] = [];
  if (x !== null) guides.push({ axis: "x", at: x.at });
  if (y !== null) guides.push({ axis: "y", at: y.at });
  return { dx: x?.shift ?? 0, dy: y?.shift ?? 0, guides };
}

/** A pointer position pulled onto the nearest targets, per axis. */
export function snapPoint(
  p: Pt,
  targets: SnapTargets,
  within: number,
): { readonly point: Pt; readonly guides: readonly Guide[] } {
  const x = nearest([p.x], targets.xs, within);
  const y = nearest([p.y], targets.ys, within);
  const guides: Guide[] = [];
  if (x !== null) guides.push({ axis: "x", at: x.at });
  if (y !== null) guides.push({ axis: "y", at: y.at });
  return { point: { x: p.x + (x?.shift ?? 0), y: p.y + (y?.shift ?? 0) }, guides };
}

/** A guide's end points, clipped to the canvas (0..w, 0..h) so it never runs past the picture. */
export function guideSegment(
  g: Guide,
  canvas: { readonly w: number; readonly h: number },
): { readonly x1: number; readonly y1: number; readonly x2: number; readonly y2: number } {
  return g.axis === "x"
    ? { x1: g.at, x2: g.at, y1: 0, y2: canvas.h }
    : { y1: g.at, y2: g.at, x1: 0, x2: canvas.w };
}
