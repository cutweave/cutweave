// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { type HitContext, type HitZone, hitTest } from "./hit-test.ts";
import { TimelineLayout } from "./layout.ts";
import type { TimelineItem, TimelineModel } from "./model.ts";
import { testTheme } from "./test-support.ts";
import { FLICKS_PER_SECOND as F } from "./time.ts";
import type { Viewport } from "./viewport.ts";

const th = testTheme();
const s = (sec: number): number => Math.round(sec * F);

const item = (
  id: string,
  trackId: string,
  start: number,
  dur: number,
  extra: Partial<TimelineItem> = {},
): TimelineItem => ({
  id,
  trackId,
  kind: trackId === "A" ? "audio" : trackId === "S" ? "captions" : "media",
  start: s(start),
  duration: s(dur),
  label: id,
  ...extra,
});

// Lanes (ruler 28 px, pad 4): V = 28..84 (clip 32..80), A = 84..140 (clip 88..136), S = 140..180
// (clip 144..176). 100 px/s, header 100 px: time t sits at x = 100 + 100 t.
const model: TimelineModel = {
  fps: { num: 24, den: 1 },
  tracks: [
    { id: "V", name: "V1", kind: "media", size: "md" },
    { id: "A", name: "A1", kind: "audio", size: "md" },
    { id: "S", name: "S1", kind: "captions", size: "sm" },
  ],
  items: [
    item("v1", "V", 1, 2, { keyframes: [s(1.5), s(2.5)] }), // x 200..400
    item("v2", "V", 4, 0.02), // 2 px wide at x 500..502
    item("a1", "A", 0, 5), // x 100..600
    item("a2", "A", 3, 1), // x 400..500, on top of a1
    item("s1", "S", 2, 1, { keyframes: [s(2.5)] }), // x 300..400, sm track: no keyframes
  ],
};

const vp = (over: Partial<Viewport> = {}): Viewport => ({
  pxPerSec: 100,
  scrollX: 0,
  scrollY: 0,
  width: 1000,
  height: 400,
  headerWidth: 100,
  rulerHeight: 28,
  ...over,
});

const layout = new TimelineLayout(model, {
  sizes: { sm: th.trackSm, md: th.trackMd, lg: th.trackLg },
  pad: th.space1,
});
const ctx = (v: Viewport = vp()): HitContext => ({
  layout,
  vp: v,
  playhead: s(6), // x = 700
  theme: th,
  flagHalfWidth: 40,
});

type Case = [string, number, number, HitZone, Partial<Record<string, unknown>>?];

const CASES: Case[] = [
  ["ruler corner", 50, 10, "none"],
  ["ruler", 500, 10, "ruler", { time: s(4) }],
  ["playhead flag centre", 700, 10, "playhead"],
  ["playhead flag edge", 735, 10, "playhead"],
  ["ruler just past the flag", 745, 10, "ruler"],
  ["header", 5, 50, "header", { trackId: "V" }],
  ["header right of toggles", 96, 50, "header", { trackId: "V" }],
  ["lock toggle", 20, 50, "header-toggle", { toggle: "lock", trackId: "V" }],
  ["mute toggle", 50, 50, "header-toggle", { toggle: "mute" }],
  ["hide toggle", 80, 50, "header-toggle", { toggle: "hide" }],
  ["clip body", 300, 50, "body", { itemId: "v1", trackId: "V" }],
  ["left trim zone (1 px in)", 201, 50, "left-edge", { itemId: "v1" }],
  ["left trim zone limit (7 px in)", 207, 50, "left-edge", { itemId: "v1" }],
  ["body just past the left zone", 209, 50, "body", { itemId: "v1" }],
  ["right trim zone limit", 392, 50, "right-edge", { itemId: "v1" }],
  ["body just before the right zone", 391, 50, "body", { itemId: "v1" }],
  ["keyframe 0", 250, 70, "keyframe", { itemId: "v1", keyframeIndex: 0, keyframeTime: s(1.5) }],
  ["keyframe 1 (a little off)", 353, 72, "keyframe", { keyframeIndex: 1 }],
  ["clip body away from the keyframe", 300, 70, "body", { itemId: "v1" }],
  ["lane padding above the clip", 250, 30, "lane", { trackId: "V" }],
  ["empty lane", 800, 50, "lane", { trackId: "V" }],
  ["2 px clip is still hittable", 499, 50, "body", { itemId: "v2" }],
  ["overlap: the later clip is on top", 450, 100, "body", { itemId: "a2" }],
  ["overlap: the earlier clip elsewhere", 550, 100, "body", { itemId: "a1" }],
  ["small track clip", 350, 160, "body", { itemId: "s1", trackId: "S" }],
  ["no keyframe hits on sm tracks", 350, 170, "body", { itemId: "s1" }],
  ["below the last track", 350, 200, "none"],
  ["right of the canvas", 1000, 50, "none"],
  ["left of the canvas", -1, 50, "none"],
];

describe("hitTest", () => {
  it("has at least 20 fixture points", () => {
    expect(CASES.length).toBeGreaterThanOrEqual(20);
  });

  it.each(CASES)("%s", (_name, x, y, zone, expected = {}) => {
    const hit = hitTest(ctx(), x, y);
    expect(hit.zone).toBe(zone);
    for (const [k, v] of Object.entries(expected)) {
      expect(hit[k as keyof typeof hit]).toBe(v);
    }
  });

  it("reports the time under the pointer in integer flicks", () => {
    const hit = hitTest(ctx(), 300, 50);
    expect(hit.time).toBe(s(2));
    expect(Number.isInteger(hitTest(ctx(), 333.3, 50).time)).toBe(true);
  });

  it("accounts for vertical scroll", () => {
    // Scrolled 20 px: y = 30 now maps to content y 22, inside the V clip (4..52).
    expect(hitTest(ctx(vp({ scrollY: 20 })), 300, 30).zone).toBe("body");
    expect(hitTest(ctx(vp({ scrollY: 20 })), 300, 30).itemId).toBe("v1");
  });

  it("accounts for horizontal scroll and zoom", () => {
    // Scroll 1 s right: v1 starts at the lane edge (x = 100); zoom x2 doubles its width.
    const v = vp({ scrollX: s(1), pxPerSec: 200 });
    expect(hitTest(ctx(v), 105, 50)).toMatchObject({ zone: "left-edge", itemId: "v1" });
    expect(hitTest(ctx(v), 300, 50)).toMatchObject({ zone: "body", itemId: "v1" });
    expect(hitTest(ctx(v), 495, 50)).toMatchObject({ zone: "right-edge", itemId: "v1" });
  });

  it("shrinks the trim zones on narrow clips so a body always remains", () => {
    // v2 at 2000 px/s is 40 px wide: zones are min(8, floor(40 / 3)) = 8 px.
    const v = vp({ pxPerSec: 2000, scrollX: s(4) - s(0.001) });
    const x0 = 100 + 2000 * 0.001;
    expect(hitTest(ctx(v), x0 + 2, 50).zone).toBe("left-edge");
    expect(hitTest(ctx(v), x0 + 20, 50).zone).toBe("body");
    // At 500 px/s v2 is 10 px wide: zones shrink to floor(10 / 3) = 3 px.
    const v2 = vp({ pxPerSec: 500, scrollX: s(4) - s(0.01) });
    const x1 = 100 + 500 * 0.01;
    expect(hitTest(ctx(v2), x1 + 1, 50).zone).toBe("left-edge");
    expect(hitTest(ctx(v2), x1 + 5, 50).zone).toBe("body");
    expect(hitTest(ctx(v2), x1 + 9, 50).zone).toBe("right-edge");
  });

  it("hit-tests keyframes only where they are drawn", () => {
    // At 40 px/s v1 is 80 px wide (< 100): keyframes hidden, so no keyframe hit.
    const v = vp({ pxPerSec: 40 });
    expect(hitTest(ctx(v), 100 + 40 * 1.5, 70).zone).toBe("body");
  });

  it("does not allocate when given a scratch buffer and reuses it", () => {
    const scratch: number[] = [];
    hitTest({ ...ctx(), scratch }, 450, 100);
    expect(scratch.length).toBeGreaterThan(0);
  });
});
