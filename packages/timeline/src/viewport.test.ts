// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { mulberry32 } from "./fixtures.ts";
import { chooseTickPlan, tickCandidates } from "./ruler.ts";
import { FLICKS_PER_SECOND } from "./time.ts";
import {
  clampScroll,
  clampZoom,
  MAX_PX_PER_SEC,
  MIN_PX_PER_SEC,
  timeToX,
  type Viewport,
  visibleTimeRange,
  xToTime,
  zoomAround,
} from "./viewport.ts";

const vp = (over: Partial<Viewport> = {}): Viewport => ({
  pxPerSec: 100,
  scrollX: 0,
  scrollY: 0,
  width: 1200,
  height: 600,
  headerWidth: 192,
  rulerHeight: 28,
  ...over,
});

const ZOOM_LEVELS = [0.5, 1, 2.5, 10, 24, 100, 480, 2400, 20_000];

describe("time <-> pixel mapping", () => {
  it.each(ZOOM_LEVELS)("round-trips integer flicks exactly at %s px/s", (pps) => {
    const rnd = mulberry32(pps * 1000);
    for (let n = 0; n < 2000; n++) {
      const scrollX = Math.floor(rnd() * 1e11);
      const v = vp({ pxPerSec: pps, scrollX });
      const t = Math.floor(rnd() * 2.5e12); // up to ~1 hour
      expect(xToTime(v, timeToX(v, t))).toBe(t);
    }
    const v = vp({ pxPerSec: pps });
    expect(xToTime(v, timeToX(v, 0))).toBe(0);
    expect(xToTime(v, timeToX(v, 2 ** 45))).toBe(2 ** 45);
  });

  it("maps the lane's left edge to scrollX and one second to pxPerSec pixels", () => {
    const v = vp({ pxPerSec: 240, scrollX: 3 * FLICKS_PER_SECOND });
    expect(timeToX(v, 3 * FLICKS_PER_SECOND)).toBe(192);
    expect(timeToX(v, 4 * FLICKS_PER_SECOND)).toBeCloseTo(192 + 240, 9);
    expect(xToTime(v, 192)).toBe(3 * FLICKS_PER_SECOND);
  });

  it("returns the visible range of the lane area", () => {
    const v = vp({ pxPerSec: 100, scrollX: FLICKS_PER_SECOND });
    const { t0, t1 } = visibleTimeRange(v);
    expect(t0).toBe(FLICKS_PER_SECOND);
    expect(t1).toBe(FLICKS_PER_SECOND + 10.08 * FLICKS_PER_SECOND);
  });
});

describe("zoom and scroll", () => {
  it("keeps the time under the anchor fixed when zooming", () => {
    const v = vp({ pxPerSec: 80, scrollX: 5 * FLICKS_PER_SECOND });
    const anchor = 640;
    const before = xToTime(v, anchor);
    const z = zoomAround(v, 2.5, anchor);
    const after = xToTime({ ...v, ...z }, anchor);
    expect(Math.abs(after - before)).toBeLessThanOrEqual(1);
    expect(z.pxPerSec).toBe(200);
  });

  it("clamps zoom to the supported range", () => {
    expect(clampZoom(1e9)).toBe(MAX_PX_PER_SEC);
    expect(clampZoom(0)).toBe(MIN_PX_PER_SEC);
    expect(zoomAround(vp({ pxPerSec: MAX_PX_PER_SEC }), 4, 400).pxPerSec).toBe(MAX_PX_PER_SEC);
  });

  it("clamps scroll to the content", () => {
    const end = 100 * FLICKS_PER_SECOND;
    expect(clampScroll(vp({ scrollX: -50, scrollY: -3 }), end, 500)).toEqual({
      scrollX: 0,
      scrollY: 0,
    });
    expect(clampScroll(vp({ scrollX: 1e15 }), end, 500).scrollX).toBe(end);
    // 600 px viewport minus 28 px ruler leaves 572 px: 400 px of tracks never scrolls.
    expect(clampScroll(vp({ scrollY: 99 }), end, 400).scrollY).toBe(0);
    expect(clampScroll(vp({ scrollY: 999 }), end, 1000).scrollY).toBe(428);
  });
});

describe("ruler ticks", () => {
  const fps = { num: 24, den: 1 };

  it("offers frame, second, minute and hour steps in ascending order", () => {
    const c = tickCandidates(fps);
    expect(c).toEqual([...c].sort((a, b) => a - b));
    expect(c[0]).toBe(29_400_000);
    expect(c).toContain(FLICKS_PER_SECOND);
    expect(c).toContain(60 * FLICKS_PER_SECOND);
  });

  it("coarsens the major step as the zoom drops and never goes below one frame", () => {
    let last = 0;
    for (const pps of [20_000, 2400, 480, 100, 24, 10, 2, 0.5, 0.05]) {
      const { step } = chooseTickPlan(pps, fps, 96);
      expect(step).toBeGreaterThanOrEqual(last);
      expect(step).toBeGreaterThanOrEqual(29_400_000);
      expect((step * pps) / FLICKS_PER_SECOND).toBeGreaterThanOrEqual(96 - 1e-9);
      last = step;
    }
  });

  it("picks one-second ticks with ten subdivisions at 100 px/s, per-frame ticks when zoomed in", () => {
    expect(chooseTickPlan(100, fps, 96)).toEqual({ step: FLICKS_PER_SECOND, divisions: 10 });
    const deep = chooseTickPlan(5000, fps, 96);
    expect(deep.step).toBe(29_400_000);
    expect(deep.divisions).toBe(1);
  });
});
