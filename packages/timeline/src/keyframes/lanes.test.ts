// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2C.05: keyframe lanes. The layout makes room under the clip, the renderer draws a mark per
// key (its shape by interpolation, collapsed when closer than 3 px, only what is visible), and
// hit-testing answers with the key or the lane that is drawn there.
import { describe, expect, it } from "vitest";
import { generateTimeline } from "../fixtures.ts";
import { hitTest } from "../hit-test.ts";
import { TimelineLayout } from "../layout.ts";
import type { TimelineItem, TimelineModel } from "../model.ts";
import { fakeCanvas, manualFrames, recordingContext, testTheme } from "../test-support.ts";
import { FLICKS_PER_SECOND as F } from "../time.ts";
import { TimelineView } from "../timeline-view.ts";
import { type ItemLanes, KEY_COLLAPSE_PX, type KeyframeLane, laneRowHeight } from "./lanes.ts";

const th = testTheme();
const s = (sec: number): number => Math.round(sec * F);
const HEADER = 4 * th.space12;
const ROW = laneRowHeight(th);
const METRICS = {
  sizes: { sm: th.trackSm, md: th.trackMd, lg: th.trackLg },
  pad: th.space1,
  laneRow: ROW,
};
/** Canvas x of time `t` at 100 px/s, scroll 0. */
const x = (t: number): number => HEADER + (t * 100) / F;

const clip = (id: string, trackId: string, start: number, dur: number): TimelineItem => ({
  id,
  trackId,
  kind: "media",
  start: s(start),
  duration: s(dur),
  label: id,
});

const MODEL: TimelineModel = {
  fps: { num: 30, den: 1 },
  tracks: [
    { id: "V2", name: "V2", kind: "media", size: "md" },
    { id: "V1", name: "V1", kind: "media", size: "md" },
  ],
  items: [clip("a", "V2", 1, 6), clip("b", "V1", 0, 4)],
};

const lane = (
  prop: string,
  secs: readonly number[],
  glyphs: readonly number[] = [],
  selected?: readonly number[],
): KeyframeLane => ({
  prop,
  label: prop,
  times: secs.map(s),
  glyphs: secs.map((_, i) => glyphs[i] ?? 0),
  ...(selected !== undefined && { selected }),
});

const LANES: readonly ItemLanes[] = [
  { itemId: "a", lanes: [lane("opacity", [2, 3, 5], [0, 1, 2]), lane("transform.rotation", [4])] },
];

function setup(model: TimelineModel = MODEL, lanes: readonly ItemLanes[] = LANES, width = 1200) {
  const rec = recordingContext();
  const frames = manualFrames();
  const view = new TimelineView(fakeCanvas(), {
    theme: th,
    context: rec.ctx,
    requestFrame: frames.requestFrame,
    cancelFrame: frames.cancelFrame,
  });
  view.resize(width, 600, 1);
  view.setModel(model);
  view.setViewState({ pxPerSec: 100, scrollX: 0 });
  view.setKeyframeLanes(lanes);
  const draw = () => {
    rec.calls.length = 0;
    view.invalidate();
    view.flush();
  };
  return { view, rec, draw };
}

describe("layout: open lanes make room in the clip's track", () => {
  it("grows the track by one row per lane, keeps the clip's height, moves the tracks below", () => {
    const plain = new TimelineLayout(MODEL, METRICS);
    const open = new TimelineLayout(MODEL, METRICS, LANES);
    expect(open.trackHeight[0]).toBe((plain.trackHeight[0] ?? 0) + 2 * ROW);
    expect(open.clipHeight(0)).toBe(plain.clipHeight(0));
    expect(open.trackTop[1]).toBe((plain.trackTop[1] ?? 0) + 2 * ROW);
    expect(open.trackHeight[1]).toBe(plain.trackHeight[1]);
    expect(open.totalHeight).toBe(plain.totalHeight + 2 * ROW);
    expect(open.hasLanes).toBe(true);
    expect(plain.hasLanes).toBe(false);
  });

  it("reuses the item index of a layout of the same model", () => {
    const plain = new TimelineLayout(MODEL, METRICS);
    const open = new TimelineLayout(MODEL, METRICS, LANES, plain);
    expect(open.indexes).toBe(plain.indexes);
    expect(open.start).toBe(plain.start);
    expect(open.laneItems[0]).toEqual([0]);
    expect(open.laneItems[1]).toEqual([]);
  });

  it("ignores lanes of items the model does not have, and items with no lane", () => {
    const layout = new TimelineLayout(MODEL, METRICS, [
      { itemId: "gone", lanes: [lane("opacity", [1])] },
      { itemId: "b", lanes: [] },
    ]);
    expect(layout.hasLanes).toBe(false);
    expect(layout.totalHeight).toBe(new TimelineLayout(MODEL, METRICS).totalHeight);
  });
});

describe("hit-testing a lane", () => {
  const { view } = setup();
  const rowY = (row: number): number => th.controlMd + th.space1 + th.trackMd + row * ROW + ROW / 2;

  it("finds the key under the pointer, with its property and time", () => {
    const hit = view.hitTest(x(s(3)) + 2, rowY(0));
    expect(hit.zone).toBe("keyframe");
    expect(hit.itemId).toBe("a");
    expect(hit.prop).toBe("opacity");
    expect(hit.keyframeIndex).toBe(1);
    expect(hit.keyframeTime).toBe(s(3));
  });

  it("answers the lane where no key is near", () => {
    const hit = view.hitTest(x(s(3.5)), rowY(1));
    expect(hit.zone).toBe("keyframe-lane");
    expect(hit.prop).toBe("transform.rotation");
    expect(hit.itemId).toBe("a");
    expect(hit.keyframeTime).toBeNull();
  });

  it("is the plain lane outside the item's span or below its rows", () => {
    expect(view.hitTest(x(s(0.5)), rowY(0)).zone).toBe("lane");
    expect(view.hitTest(x(s(7.5)), rowY(1)).zone).toBe("lane");
  });

  it("still hits the clip above the lanes and the track below them", () => {
    expect(view.hitTest(x(s(3)), th.controlMd + th.space1 + 10).zone).toBe("body");
    const below = view.hitTest(x(s(1)), th.controlMd + (th.trackMd + 2 * th.space1 + 2 * ROW) + 10);
    expect(below.itemId).toBe("b");
  });

  it("a diamond of the clip's own key row has no property", () => {
    const layout = new TimelineLayout(
      { ...MODEL, items: [{ ...clip("a", "V2", 1, 6), keyframes: [s(2)] }] },
      METRICS,
    );
    const hit = hitTest(
      {
        layout,
        vp: {
          pxPerSec: 100,
          scrollX: 0,
          scrollY: 0,
          width: 1200,
          height: 600,
          headerWidth: HEADER,
          rulerHeight: th.controlMd,
        },
        playhead: 0,
        theme: th,
        flagHalfWidth: 8,
      },
      x(s(2)),
      th.controlMd + th.space1 + th.trackMd - th.space1_5 - th.space1,
    );
    expect(hit.zone).toBe("keyframe");
    expect(hit.prop).toBeNull();
  });
});

describe("drawing lanes", () => {
  it("draws the strip, the property names and one mark per key, batched by shape", () => {
    const { view, rec, draw } = setup();
    draw();
    expect(rec.texts()).toContain("opacity");
    expect(rec.texts()).toContain("transform.rotation");
    expect(view.stats.visibleKeyframes).toBe(4);
    // linear diamonds (2 keys), a hold square and a bezier circle: three fills, not four.
    const arcs = rec.named("arc");
    expect(arcs).toHaveLength(1);
    expect(arcs[0]?.args[0]).toBeCloseTo(x(s(5)));
  });

  it("fills selected keys in marigold", () => {
    const { rec, draw } = setup(MODEL, [
      { itemId: "a", lanes: [lane("opacity", [2, 3, 5], [], [1])] },
    ]);
    draw();
    const fills = rec.named("fill").map((c) => c.fillStyle);
    expect(fills).toContain(th.marigold);
    expect(fills).toContain(th.textPrimary);
  });

  it("collapses keys closer than 3 px into one mark", () => {
    // 1,000 keys over one second at 100 px/s: 0.1 px apart.
    const secs = Array.from({ length: 1000 }, (_, i) => 2 + i / 1000);
    const { view, draw } = setup(MODEL, [{ itemId: "a", lanes: [lane("opacity", secs)] }]);
    draw();
    const marks = view.stats.visibleKeyframes;
    expect(marks).toBeLessThanOrEqual(Math.ceil(100 / KEY_COLLAPSE_PX) + 1);
    expect(marks).toBeGreaterThan(20);
    // A collapsed run still answers a real key to the pointer.
    const hit = view.hitTest(x(s(2.5)), th.controlMd + th.space1 + th.trackMd + ROW / 2);
    expect(hit.zone).toBe("keyframe");
    expect(Math.abs((hit.keyframeTime ?? 0) - s(2.5))).toBeLessThan(s(0.01));
  });

  it("a selected key inside a collapsed run still shows as selected", () => {
    const secs = Array.from({ length: 100 }, (_, i) => 2 + i / 1000);
    const { rec, draw } = setup(MODEL, [{ itemId: "a", lanes: [lane("opacity", secs, [], [57])] }]);
    draw();
    expect(rec.named("fill").map((c) => c.fillStyle)).toContain(th.marigold);
  });

  it("walks only the keys in view (virtualized)", () => {
    const secs = Array.from({ length: 50_000 }, (_, i) => 1 + (i * 6) / 50_000);
    const { view, draw } = setup(MODEL, [{ itemId: "a", lanes: [lane("opacity", secs)] }]);
    view.setViewState({ pxPerSec: 2000, scrollX: s(3) });
    draw();
    // 0.5 s of lane is in view (1,000 px at 2,000 px/s): ~4,170 keys, 0.24 px apart.
    expect(view.stats.visibleKeyframes).toBeLessThanOrEqual(Math.ceil(1000 / KEY_COLLAPSE_PX) + 2);
    expect(view.stats.visibleKeyframes).toBeGreaterThan(100);
  });

  it("draws nothing for lanes scrolled out of view", () => {
    const { view, draw } = setup();
    view.setViewState({ scrollX: s(20) });
    draw();
    expect(view.stats.visibleKeyframes).toBe(0);
  });

  it("the stress project with every clip's lanes open keeps the mark count bounded", () => {
    const model = generateTimeline();
    const lanes: ItemLanes[] = model.items.map((item) => ({
      itemId: item.id,
      lanes: [
        {
          prop: "opacity",
          label: "Opacity",
          times: item.keyframes ?? [],
          glyphs: new Uint8Array(item.keyframes?.length ?? 0),
        },
      ],
    }));
    const { view, draw } = setup(model, lanes, 1600);
    view.setViewState({ pxPerSec: 0.5, scrollX: 0 });
    draw();
    // Whole project in view: far fewer marks than the 50,000 keys.
    const tracks = view.stats.tracksDrawn;
    expect(tracks).toBeGreaterThan(0);
    expect(view.stats.visibleKeyframes).toBeLessThanOrEqual(
      tracks * Math.ceil(1600 / KEY_COLLAPSE_PX) * 2,
    );
  });
});

describe("TimelineView.setKeyframeLanes", () => {
  it("redraws when lanes open and close, and not when nothing changed", () => {
    const { view } = setup();
    view.flush();
    view.setKeyframeLanes(LANES);
    expect(view.flush()).toBe(false);
    view.setKeyframeLanes([]);
    expect(view.flush()).toBe(true);
    view.setKeyframeLanes([]);
    expect(view.flush()).toBe(false);
  });

  it("keeps the lanes across a model change", () => {
    const { view, draw } = setup();
    view.setModel({ ...MODEL, items: [...MODEL.items] });
    draw();
    expect(view.stats.visibleKeyframes).toBe(4);
  });
});
