// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2C.07: the graph editor's geometry. The viewport maps time and value to pixels and back,
// zoom keeps what is under the pointer, fit shows every key, the rulers step in frames, seconds
// and round values, bezier handles are points of the graph, and the tangent modes move the
// handle across a key.
import { describe, expect, it } from "vitest";
import {
  boxOf,
  constrainAxis,
  denormalizeDelta,
  fitExtent,
  type GraphViewport,
  handleNumbers,
  handlePoint,
  keysInBox,
  lowerBound,
  nearestKey,
  niceStep,
  normalize,
  oppositeOfIn,
  oppositeOfOut,
  type PlotRect,
  panBy,
  type Segment,
  tAt,
  ticks,
  timeStep,
  vAt,
  valueRange,
  xOf,
  yOf,
  zoomAbout,
} from "./geometry.ts";

const VP: GraphViewport = { t0: 0, t1: 1000, v0: -50, v1: 150 };
const PLOT: PlotRect = { left: 60, top: 20, width: 500, height: 200 };
const LIMITS = { minTime: 1, maxTime: 1e9, minValue: 1e-6, maxValue: 1e9 };

describe("the viewport", () => {
  it("maps time and value to pixels and back", () => {
    expect(xOf(VP, PLOT, 0)).toBe(60);
    expect(xOf(VP, PLOT, 1000)).toBe(560);
    expect(yOf(VP, PLOT, 150)).toBe(20);
    expect(yOf(VP, PLOT, -50)).toBe(220);
    expect(tAt(VP, PLOT, xOf(VP, PLOT, 123))).toBeCloseTo(123, 9);
    expect(vAt(VP, PLOT, yOf(VP, PLOT, 42))).toBeCloseTo(42, 9);
  });

  it("zooms about the pointer: what is under it stays under it", () => {
    const x = 200;
    const y = 90;
    const t = tAt(VP, PLOT, x);
    const v = vAt(VP, PLOT, y);
    const zoomed = zoomAbout(VP, PLOT, x, y, 2, 4, LIMITS);
    expect(zoomed.t1 - zoomed.t0).toBeCloseTo(500, 9);
    expect(zoomed.v1 - zoomed.v0).toBeCloseTo(50, 9);
    expect(tAt(zoomed, PLOT, x)).toBeCloseTo(t, 9);
    expect(vAt(zoomed, PLOT, y)).toBeCloseTo(v, 9);
  });

  it("never zooms past its limits", () => {
    const zoomed = zoomAbout(VP, PLOT, 200, 90, 1e12, 1e-12, LIMITS);
    expect(zoomed.t1 - zoomed.t0).toBe(1);
    expect(zoomed.v1 - zoomed.v0).toBe(1e9);
  });

  it("pans with the pointer", () => {
    const moved = panBy(VP, PLOT, 50, -20);
    // Dragging right shows earlier times; dragging up shows lower values.
    expect(moved.t0).toBeCloseTo(-100, 9);
    expect(moved.v0).toBeCloseTo(-70, 9);
    expect(moved.t1 - moved.t0).toBeCloseTo(1000, 9);
  });

  it("fits an extent with a margin, and gives a single key or a flat curve a range", () => {
    const vp = fitExtent({ tMin: 100, tMax: 300, vMin: 0, vMax: 10 }, 1000, 1);
    expect(vp).toEqual({ t0: 80, t1: 320, v0: -1, v1: 11 });
    const flat = fitExtent({ tMin: 100, tMax: 100, vMin: 5, vMax: 5 }, 1000, 1);
    expect(flat.t1 - flat.t0).toBeGreaterThan(0);
    expect(flat.v1 - flat.v0).toBeGreaterThan(0);
    expect((flat.t0 + flat.t1) / 2).toBeCloseTo(100, 9);
    expect((flat.v0 + flat.v1) / 2).toBeCloseTo(5, 9);
  });
});

describe("the rulers", () => {
  it("steps values by 1, 2 or 5 times a power of ten", () => {
    expect(niceStep(100, 10)).toBe(10);
    expect(niceStep(100, 7)).toBe(20);
    expect(niceStep(1, 4)).toBe(0.5);
    expect(niceStep(0.03, 3)).toBeCloseTo(0.01, 12);
    expect(ticks(-25, 25, 10)).toEqual([-20, -10, 0, 10, 20]);
    expect(ticks(0, 1, 0)).toEqual([]);
    expect(ticks(0, 1e9, 1, 50)).toHaveLength(50);
  });

  it("steps time in frames while they are wide enough, then in seconds", () => {
    const second = 1000;
    const frame = 40;
    expect(timeStep(30, second, frame)).toBe(40); // one frame
    expect(timeStep(150, second, frame)).toBe(200); // five frames
    expect(timeStep(450, second, frame)).toBe(1000); // one second
    expect(timeStep(4000, second, frame)).toBe(5000);
    expect(timeStep(40_000, second, frame)).toBe(60_000);
  });
});

describe("bezier handles", () => {
  const seg: Segment = { t0: 1000, v0: 10, t1: 3000, v1: 50 };
  const ease = { x1: 0.42, y1: 0, x2: 0.58, y2: 1 };

  it("places the handles of a segment in the graph", () => {
    const out = handlePoint(seg, ease, "out");
    expect(out.t).toBeCloseTo(1840, 9);
    expect(out.v).toBe(10);
    const inn = handlePoint(seg, ease, "in");
    expect(inn.t).toBeCloseTo(2160, 9);
    expect(inn.v).toBe(50);
  });

  it("reads a dragged handle back as bezier numbers, within the engine's limits", () => {
    expect(handleNumbers(seg, { t: 1500, v: 30 }, 0)).toEqual({ x: 0.25, y: 0.5 });
    expect(handleNumbers(seg, { t: 500, v: 30 }, 0).x).toBe(0);
    expect(handleNumbers(seg, { t: 9000, v: 30 }, 0).x).toBe(1);
    expect(handleNumbers(seg, { t: 1500, v: 1e6 }, 0).y).toBe(10);
    expect(handleNumbers(seg, { t: 1500, v: -1e6 }, 0).y).toBe(-10);
    // Round trip.
    const p = handlePoint(seg, ease, "in");
    expect(handleNumbers(seg, p, 0)).toEqual({ x: 0.58, y: 1 });
  });

  it("keeps a handle's progress on a flat segment, where there is no height to measure", () => {
    const flat: Segment = { t0: 0, v0: 5, t1: 100, v1: 5 };
    expect(handleNumbers(flat, { t: 25, v: 99 }, 0.3)).toEqual({ x: 0.25, y: 0.3 });
  });
});

describe("tangent modes", () => {
  // Key at (2000, 100): the segment before it rises 100 in 1000, the one after falls 50 in 2000.
  const prev: Segment = { t0: 1000, v0: 0, t1: 2000, v1: 100 };
  const next: Segment = { t0: 2000, v0: 100, t1: 4000, v1: 50 };

  it("free leaves the other handle alone", () => {
    expect(oppositeOfOut("free", next, 0.25, 0.5, prev, 0.6, 1)).toBeNull();
    expect(oppositeOfIn("free", prev, 0.6, 1, next, 0.25, 0.5)).toBeNull();
  });

  it("aligned turns the other handle onto the same line and keeps its reach", () => {
    // Out handle: 500 later, 25 lower (slope -0.05 per unit).
    const got = oppositeOfOut("aligned", next, 0.25, 0.5, prev, 0.6, 1);
    // The in handle reaches 400 back (x2 = 0.6), so it sits 20 higher than the key: y2 = 1.2.
    expect(got).toEqual({ x: 0.6, y: 1.2 });
    const out = handlePoint(next, { x1: 0.25, y1: 0.5, x2: 0, y2: 0 }, "out");
    const inn = handlePoint(prev, { x1: 0, y1: 0, x2: got?.x ?? 0, y2: got?.y ?? 0 }, "in");
    const slopeOut = (out.v - 100) / (out.t - 2000);
    const slopeIn = (100 - inn.v) / (2000 - inn.t);
    expect(slopeIn).toBeCloseTo(slopeOut, 9);
  });

  it("mirrored also gives the other handle the same reach", () => {
    const got = oppositeOfOut("mirrored", next, 0.25, 0.5, prev, 0.6, 1);
    // 500 back of a 1000 long segment, 25 higher than the key.
    expect(got).toEqual({ x: 0.5, y: 1.25 });
  });

  it("works from the in handle too", () => {
    // In handle 400 back and 20 below the key: slope 0.05.
    const aligned = oppositeOfIn("aligned", prev, 0.6, 0.8, next, 0.25, 0.5);
    // The out handle reaches 500 ahead, so it rises 25: of a segment that falls 50, y1 = -0.5.
    expect(aligned).toEqual({ x: 0.25, y: -0.5 });
    const mirrored = oppositeOfIn("mirrored", prev, 0.6, 0.8, next, 0.25, 0.5);
    expect(mirrored).toEqual({ x: 0.2, y: -0.4 });
  });

  it("does nothing for a handle without a direction, and clamps to the engine's limits", () => {
    expect(oppositeOfOut("aligned", next, 0, 0.5, prev, 0.6, 1)).toBeNull();
    const steep = oppositeOfOut("aligned", next, 0.01, 10, prev, 0.6, 1);
    expect(steep?.y).toBe(10);
    // A reach longer than the other segment stops at its far key.
    const long = oppositeOfOut("mirrored", next, 1, 0, prev, 0.6, 1);
    expect(long?.x).toBe(0);
  });

  it("keeps the other handle's progress when its segment is flat", () => {
    const flatPrev: Segment = { t0: 1000, v0: 100, t1: 2000, v1: 100 };
    expect(oppositeOfOut("mirrored", next, 0.25, 0.5, flatPrev, 0.6, 0.7)).toEqual({
      x: 0.5,
      y: 0.7,
    });
  });
});

describe("what is under the pointer", () => {
  const times = Float64Array.from([0, 250, 500, 750, 1000]);
  const values = [0, 100, 50, 100, 0];
  const toY = (i: number): number => yOf(VP, PLOT, values[i] as number);

  it("finds the first key at or after a time", () => {
    expect(lowerBound(times, -1)).toBe(0);
    expect(lowerBound(times, 250)).toBe(1);
    expect(lowerBound(times, 251)).toBe(2);
    expect(lowerBound(times, 5000)).toBe(5);
  });

  it("finds the nearest key within reach", () => {
    const x = xOf(VP, PLOT, 500);
    const y = yOf(VP, PLOT, 50);
    expect(nearestKey(times, VP, PLOT, toY, x + 3, y - 2, 8)?.index).toBe(2);
    expect(nearestKey(times, VP, PLOT, toY, x + 30, y, 8)).toBeNull();
    expect(nearestKey(times, VP, PLOT, toY, x, y + 20, 8)).toBeNull();
  });

  it("collects the keys inside a box, whichever way it was dragged", () => {
    const box = boxOf(xOf(VP, PLOT, 800), yOf(VP, PLOT, 40), xOf(VP, PLOT, 200), yOf(VP, PLOT, 120));
    expect(keysInBox(times, VP, PLOT, toY, box)).toEqual([1, 2, 3]);
    const low = boxOf(xOf(VP, PLOT, 200), yOf(VP, PLOT, 60), xOf(VP, PLOT, 800), yOf(VP, PLOT, 40));
    expect(keysInBox(times, VP, PLOT, toY, low)).toEqual([2]);
  });

  it("keeps a constrained drag to the axis it moved further along", () => {
    expect(constrainAxis(10, 3)).toEqual({ dx: 10, dy: 0 });
    expect(constrainAxis(-2, 7)).toEqual({ dx: 0, dy: 7 });
  });
});

describe("the normalized view", () => {
  it("runs every curve from its lowest key to its highest", () => {
    const range = valueRange([20, -20, 60]);
    expect(range).toEqual({ lo: -20, hi: 60 });
    expect(normalize(-20, range)).toBe(0);
    expect(normalize(60, range)).toBe(1);
    expect(normalize(20, range)).toBe(0.5);
    expect(denormalizeDelta(0.25, range)).toBe(20);
  });

  it("puts a flat curve in the middle", () => {
    const range = valueRange([7, 7]);
    expect(normalize(7, range)).toBe(0.5);
    expect(denormalizeDelta(0.25, range)).toBe(0.25);
    expect(valueRange([])).toEqual({ lo: 0, hi: 1 });
  });
});
