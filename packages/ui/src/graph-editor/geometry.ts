// SPDX-License-Identifier: GPL-3.0-or-later
// The graph editor's geometry (WP-2C.07): the window of time and value the plot shows, the ticks
// of its two rulers, the bezier handles of a segment as points of the graph, what the tangent
// modes do to the handle across a key, and what is under a pointer. Pure numbers: no DOM, no
// canvas. Positions are CSS pixels inside the canvas; times and values are the caller's units
// (the editor passes sequence time in flicks and the property's own values).

/** The time and value range the plot shows. `v1` (the top) is larger than `v0`. */
export interface GraphViewport {
  readonly t0: number;
  readonly t1: number;
  readonly v0: number;
  readonly v1: number;
}

/** Where the curves are drawn inside the canvas (beside the value ruler, under the time ruler). */
export interface PlotRect {
  readonly left: number;
  readonly top: number;
  readonly width: number;
  readonly height: number;
}

export const xOf = (vp: GraphViewport, plot: PlotRect, t: number): number =>
  plot.left + ((t - vp.t0) / (vp.t1 - vp.t0)) * plot.width;

export const yOf = (vp: GraphViewport, plot: PlotRect, v: number): number =>
  plot.top + ((vp.v1 - v) / (vp.v1 - vp.v0)) * plot.height;

export const tAt = (vp: GraphViewport, plot: PlotRect, x: number): number =>
  vp.t0 + ((x - plot.left) / plot.width) * (vp.t1 - vp.t0);

export const vAt = (vp: GraphViewport, plot: PlotRect, y: number): number =>
  vp.v1 - ((y - plot.top) / plot.height) * (vp.v1 - vp.v0);

/** The smallest spans a viewport may show (so a zoom never divides by zero). */
export interface ZoomLimits {
  readonly minTime: number;
  readonly maxTime: number;
  readonly minValue: number;
  readonly maxValue: number;
}

const clamp = (x: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, x));

/**
 * Zooms about the point under (`x`, `y`): `ft` and `fv` multiply the pixels per unit of each
 * axis (above 1 zooms in). The time and value under the pointer stay under it.
 */
export function zoomAbout(
  vp: GraphViewport,
  plot: PlotRect,
  x: number,
  y: number,
  ft: number,
  fv: number,
  limits: ZoomLimits,
): GraphViewport {
  const t = tAt(vp, plot, x);
  const v = vAt(vp, plot, y);
  const spanT = clamp((vp.t1 - vp.t0) / ft, limits.minTime, limits.maxTime);
  const spanV = clamp((vp.v1 - vp.v0) / fv, limits.minValue, limits.maxValue);
  const ax = (x - plot.left) / plot.width;
  const ay = (y - plot.top) / plot.height;
  const t0 = t - ax * spanT;
  const v1 = v + ay * spanV;
  return { t0, t1: t0 + spanT, v0: v1 - spanV, v1 };
}

/** The viewport moved by a drag of (`dx`, `dy`) pixels: what was under the pointer follows it. */
export function panBy(vp: GraphViewport, plot: PlotRect, dx: number, dy: number): GraphViewport {
  const dt = (-dx / plot.width) * (vp.t1 - vp.t0);
  const dv = (dy / plot.height) * (vp.v1 - vp.v0);
  return { t0: vp.t0 + dt, t1: vp.t1 + dt, v0: vp.v0 + dv, v1: vp.v1 + dv };
}

/** The extent of some points. */
export interface Extent {
  readonly tMin: number;
  readonly tMax: number;
  readonly vMin: number;
  readonly vMax: number;
}

/** The share of the plot left free around what is fitted. */
export const FIT_MARGIN = 0.1;

/**
 * The viewport that shows `extent` with a margin. A single time or a flat value still gets a
 * range: `flatTime` and `flatValue` are the spans shown around it.
 */
export function fitExtent(extent: Extent, flatTime: number, flatValue: number): GraphViewport {
  let { tMin, tMax, vMin, vMax } = extent;
  if (!(tMax > tMin)) {
    tMin -= flatTime / 2;
    tMax += flatTime / 2;
  }
  if (!(vMax > vMin)) {
    const half = Math.max(flatValue, Math.abs(vMin) * FIT_MARGIN) / 2;
    vMin -= half;
    vMax += half;
  }
  const mt = (tMax - tMin) * FIT_MARGIN;
  const mv = (vMax - vMin) * FIT_MARGIN;
  return { t0: tMin - mt, t1: tMax + mt, v0: vMin - mv, v1: vMax + mv };
}

// ---- rulers ---------------------------------------------------------------------------------

/** The step of a ruler: a 1, 2 or 5 times a power of ten, at least `minStep`. */
export function niceStep(span: number, maxTicks: number, minStep = 0): number {
  if (!(span > 0) || maxTicks < 1) return Math.max(1, minStep);
  const raw = span / maxTicks;
  const pow = 10 ** Math.floor(Math.log10(raw));
  const unit = raw / pow;
  const nice = unit <= 1 ? 1 : unit <= 2 ? 2 : unit <= 5 ? 5 : 10;
  return Math.max(nice * pow, minStep);
}

/** The multiples of `step` in `[lo, hi]` (at most `cap`, so a bad step cannot hang a frame). */
export function ticks(lo: number, hi: number, step: number, cap = 200): number[] {
  const out: number[] = [];
  if (!(step > 0) || !(hi >= lo)) return out;
  const first = Math.ceil(lo / step);
  const last = Math.floor(hi / step);
  for (let i = first; i <= last && out.length < cap; i++) out.push(i * step);
  return out;
}

/** Tick steps of the time ruler, in frames and then in seconds. */
const FRAME_STEPS = [1, 2, 5, 10] as const;
const SECOND_STEPS = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 1800, 3600] as const;

/**
 * The step of the time ruler: whole frames while frames are wide enough, then whole seconds.
 * `second` and `frame` are in time units; `unitsPerLabel` is the room one label needs.
 */
export function timeStep(unitsPerLabel: number, second: number, frame: number): number {
  for (const n of FRAME_STEPS) {
    const step = n * frame;
    if (step >= unitsPerLabel && step < second) return step;
  }
  for (const n of SECOND_STEPS) {
    if (n * second >= unitsPerLabel) return n * second;
  }
  return niceStep(unitsPerLabel, 1, second * 3600);
}

// ---- bezier handles -------------------------------------------------------------------------

/** A segment between two keys, in graph units. */
export interface Segment {
  readonly t0: number;
  readonly v0: number;
  readonly t1: number;
  readonly v1: number;
}

/** A bezier's four numbers, as the engine stores them (`Interp::Bezier`). */
export interface BezierNumbers {
  readonly x1: number;
  readonly y1: number;
  readonly x2: number;
  readonly y2: number;
}

export interface GraphPoint {
  readonly t: number;
  readonly v: number;
}

/** The engine accepts handle progress from -10 to 10 (KF-5). */
export const BEZIER_Y_LIMIT = 10;

export type HandleSide = "out" | "in";

/** The handle leaving the segment's first key (`out`) or arriving at its second (`in`). */
export function handlePoint(seg: Segment, b: BezierNumbers, side: HandleSide): GraphPoint {
  const dt = seg.t1 - seg.t0;
  const dv = seg.v1 - seg.v0;
  return side === "out"
    ? { t: seg.t0 + b.x1 * dt, v: seg.v0 + b.y1 * dv }
    : { t: seg.t0 + b.x2 * dt, v: seg.v0 + b.y2 * dv };
}

const round6 = (x: number): number => Math.round(x * 1e6) / 1e6;

/**
 * The bezier numbers of a handle dragged to `point`: time within the segment, progress within
 * the engine's limits. A segment between two equal values has no height to measure progress
 * against, so the handle keeps its `y` (`keepY`).
 */
export function handleNumbers(
  seg: Segment,
  point: GraphPoint,
  keepY: number,
): { x: number; y: number } {
  const dt = seg.t1 - seg.t0;
  const dv = seg.v1 - seg.v0;
  const x = dt > 0 ? clamp((point.t - seg.t0) / dt, 0, 1) : 0;
  const y =
    dv === 0 ? keepY : clamp((point.v - seg.v0) / dv, -BEZIER_Y_LIMIT, BEZIER_Y_LIMIT);
  return { x: round6(x), y: round6(y) };
}

/** What dragging one handle of a key does to the handle on its other side. */
export type TangentMode = "free" | "aligned" | "mirrored";

/**
 * The out handle of a key (the start of `next`) is now (`x1`, `y1`): where its in handle (the
 * end of `prev`, now at (`x2`, `y2`)) goes. `aligned` turns it onto the same line and keeps how
 * far it reaches in time; `mirrored` also gives it the same reach. `null`: it stays (free mode,
 * or the dragged handle has no direction).
 */
export function oppositeOfOut(
  mode: TangentMode,
  next: Segment,
  x1: number,
  y1: number,
  prev: Segment,
  x2: number,
  y2: number,
): { x: number; y: number } | null {
  if (mode === "free") return null;
  const reach = x1 * (next.t1 - next.t0);
  const rise = y1 * (next.v1 - next.v0);
  if (!(reach > 0)) return null;
  const slope = rise / reach;
  const dtP = prev.t1 - prev.t0;
  const dvP = prev.v1 - prev.v0;
  if (!(dtP > 0)) return null;
  const back = mode === "mirrored" ? Math.min(reach, dtP) : (1 - x2) * dtP;
  if (!(back > 0)) return null;
  const y = dvP === 0 ? y2 : clamp(1 - (slope * back) / dvP, -BEZIER_Y_LIMIT, BEZIER_Y_LIMIT);
  return { x: round6(1 - back / dtP), y: round6(y) };
}

/** The in handle of a key (the end of `prev`) is now (`x2`, `y2`): where its out handle goes. */
export function oppositeOfIn(
  mode: TangentMode,
  prev: Segment,
  x2: number,
  y2: number,
  next: Segment,
  x1: number,
  y1: number,
): { x: number; y: number } | null {
  if (mode === "free") return null;
  const reach = (1 - x2) * (prev.t1 - prev.t0);
  const rise = (1 - y2) * (prev.v1 - prev.v0);
  if (!(reach > 0)) return null;
  const slope = rise / reach;
  const dtN = next.t1 - next.t0;
  const dvN = next.v1 - next.v0;
  if (!(dtN > 0)) return null;
  const ahead = mode === "mirrored" ? Math.min(reach, dtN) : x1 * dtN;
  if (!(ahead > 0)) return null;
  const y = dvN === 0 ? y1 : clamp((slope * ahead) / dvN, -BEZIER_Y_LIMIT, BEZIER_Y_LIMIT);
  return { x: round6(ahead / dtN), y: round6(y) };
}

// ---- what is where --------------------------------------------------------------------------

/** The first index whose time is at least `t` (`times` ascending). */
export function lowerBound(times: ArrayLike<number>, t: number): number {
  let lo = 0;
  let hi = times.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if ((times[mid] as number) < t) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/**
 * The key of a curve nearest to (`x`, `y`) within `radius` pixels, or -1. `toY` gives the y of
 * key `i`; only the keys whose x is within reach are measured.
 */
export function nearestKey(
  times: ArrayLike<number>,
  vp: GraphViewport,
  plot: PlotRect,
  toY: (index: number) => number,
  x: number,
  y: number,
  radius: number,
): { index: number; distance: number } | null {
  const reach = (radius / plot.width) * (vp.t1 - vp.t0);
  const t = tAt(vp, plot, x);
  let best: { index: number; distance: number } | null = null;
  for (let i = lowerBound(times, t - reach); i < times.length; i++) {
    const kt = times[i] as number;
    if (kt > t + reach) break;
    const d = Math.hypot(xOf(vp, plot, kt) - x, toY(i) - y);
    if (d <= radius && (best === null || d < best.distance)) best = { index: i, distance: d };
  }
  return best;
}

/** A box dragged between two corners, as left, top, right and bottom. */
export interface Box {
  readonly x0: number;
  readonly y0: number;
  readonly x1: number;
  readonly y1: number;
}

export function boxOf(ax: number, ay: number, bx: number, by: number): Box {
  return { x0: Math.min(ax, bx), y0: Math.min(ay, by), x1: Math.max(ax, bx), y1: Math.max(ay, by) };
}

/** The keys of a curve inside the box. */
export function keysInBox(
  times: ArrayLike<number>,
  vp: GraphViewport,
  plot: PlotRect,
  toY: (index: number) => number,
  box: Box,
): number[] {
  const out: number[] = [];
  const tLo = tAt(vp, plot, box.x0);
  const tHi = tAt(vp, plot, box.x1);
  for (let i = lowerBound(times, tLo); i < times.length; i++) {
    if ((times[i] as number) > tHi) break;
    const y = toY(i);
    if (y >= box.y0 && y <= box.y1) out.push(i);
  }
  return out;
}

/** Shift held: the drag keeps to the axis it has moved further along. */
export function constrainAxis(dx: number, dy: number): { dx: number; dy: number } {
  return Math.abs(dx) >= Math.abs(dy) ? { dx, dy: 0 } : { dx: 0, dy };
}

// ---- the normalized view --------------------------------------------------------------------

/** The value range of a curve's keys. */
export function valueRange(values: ArrayLike<number>): { lo: number; hi: number } {
  let lo = Number.POSITIVE_INFINITY;
  let hi = Number.NEGATIVE_INFINITY;
  for (let i = 0; i < values.length; i++) {
    const v = values[i] as number;
    if (v < lo) lo = v;
    if (v > hi) hi = v;
  }
  if (!(hi >= lo)) return { lo: 0, hi: 1 };
  return { lo, hi };
}

/**
 * A value in the normalized view: 0 at the curve's lowest key and 1 at its highest, so curves
 * of different units share one graph. A flat curve sits in the middle.
 */
export function normalize(v: number, range: { lo: number; hi: number }): number {
  return range.hi > range.lo ? (v - range.lo) / (range.hi - range.lo) : 0.5;
}

/** What a move of `dn` in the normalized view is in the curve's own values. */
export function denormalizeDelta(dn: number, range: { lo: number; hi: number }): number {
  return range.hi > range.lo ? dn * (range.hi - range.lo) : dn;
}
