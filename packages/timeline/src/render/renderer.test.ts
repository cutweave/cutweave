// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2A.09: what the renderer draws (markers, snap guides by kind, linked badges, track styles
// and heights, time formats), its zoom range and smooth zoom, and that media never block a frame.
import { afterEach, describe, expect, it, vi } from "vitest";
import { type FilmstripRequest, MediaCaches, type MediaLoaders, zoomBucket } from "../cache.ts";
import { generateTimeline } from "../fixtures.ts";
import { TimelineLayout } from "../layout.ts";
import type { TimelineItem, TimelineModel, TimelineTrack } from "../model.ts";
import { fakeCanvas, manualFrames, recordingContext, testTheme } from "../test-support.ts";
import { FLICKS_PER_SECOND as F, frameDurationFlicks } from "../time.ts";
import { TimelineView, type TimelineViewOptions } from "../timeline-view.ts";
import { MAX_PX_PER_SEC } from "../viewport.ts";

const th = testTheme();
const s = (sec: number): number => Math.round(sec * F);
const tick = (): Promise<void> => new Promise((r) => setTimeout(r, 0));
/** Header column of a view built with the test theme: 4 x space-12. */
const HEADER = 4 * th.space12;

const track = (id: string, extra: Partial<TimelineTrack> = {}): TimelineTrack => ({
  id,
  name: id,
  kind: "media",
  size: "md",
  ...extra,
});
const clip = (
  id: string,
  trackId: string,
  start: number,
  dur: number,
  extra = {},
): TimelineItem => ({
  id,
  trackId,
  kind: "media",
  start: s(start),
  duration: s(dur),
  label: `clip ${id}`,
  ...extra,
});

function setup(model: TimelineModel, opts: TimelineViewOptions = {}, width = 1200, height = 400) {
  const rec = recordingContext();
  const frames = manualFrames();
  let clock = 0;
  const view = new TimelineView(fakeCanvas(), {
    theme: th,
    context: rec.ctx,
    requestFrame: frames.requestFrame,
    cancelFrame: frames.cancelFrame,
    now: () => clock,
    ...opts,
  });
  view.resize(width, height, 1);
  view.setModel(model);
  view.setViewState({ pxPerSec: 100, scrollX: 0, playhead: s(9) });
  const draw = () => {
    rec.calls.length = 0;
    view.invalidate();
    view.flush();
  };
  return { view, rec, frames, draw, advance: (ms: number) => (clock += ms) };
}

const x = (t: number): number => HEADER + (t * 100) / F; // at 100 px/s, scroll 0

describe("sequence markers in the ruler", () => {
  const model: TimelineModel = {
    fps: { num: 24, den: 1 },
    tracks: [track("V")],
    items: [clip("a", "V", 0, 4)],
    markers: [
      { id: "far", t: s(100), label: "", kind: "user" },
      { id: "m1", t: s(1), label: "Intro", color: "#ff0000", kind: "user" },
      { id: "range", t: s(3), duration: s(2), label: "Range", kind: "user" },
      { id: "ch", t: s(2), label: "Chapter", color: "#00ff00", kind: "chapter" },
      { id: "b1", t: s(5), label: "", kind: "beat" },
      { id: "b2", t: s(5.01), label: "", kind: "beat" }, // 1 px after b1: thinned
      { id: "b3", t: s(6), label: "", kind: "beat" },
    ],
  };

  it("draws visible markers in their colors, range bars dimmed, and thins dense beats", () => {
    const { view, rec, draw } = setup(model);
    draw();
    expect(view.stats.markers).toBe(5); // m1, ch, range, b1, b3 (b2 thinned, far off screen)
    const fills = rec.named("fill");
    expect(fills.some((c) => c.fillStyle === "#ff0000")).toBe(true);
    expect(fills.some((c) => c.fillStyle === "#00ff00")).toBe(true);
    // Default color is the marigold token; the range bar is drawn at the disabled opacity.
    expect(
      fills.some((c) => c.fillStyle === "var(--marigold)" && c.globalAlpha === th.opacityDisabled),
    ).toBe(true);
    expect(fills.some((c) => c.fillStyle === "var(--marigold)" && c.globalAlpha === 1)).toBe(true);
  });

  it("finds the marker under the pointer for the interaction layer", () => {
    const { view } = setup(model);
    const pinY = th.controlMd - 6;
    expect(view.markerAt(x(s(1)), pinY)?.id).toBe("m1");
    expect(view.markerAt(x(s(1)) + 3, pinY)?.id).toBe("m1");
    expect(view.markerAt(x(s(4)), th.controlMd - 2)?.id).toBe("range"); // inside its bar
    expect(view.markerAt(x(s(1)), 3)).toBeNull(); // above the pins: the timecode labels
    expect(view.markerAt(x(s(1)), th.controlMd + 10)).toBeNull(); // in the lanes
    expect(view.markerAt(x(s(8)), pinY)).toBeNull();
  });

  it("sorts and culls through the layout", () => {
    const layout = new TimelineLayout(model, {
      sizes: { sm: th.trackSm, md: th.trackMd, lg: th.trackLg },
      pad: th.space1,
    });
    expect(layout.markers.map((m) => m.id)).toEqual(["m1", "ch", "range", "b1", "b2", "b3", "far"]);
    expect(layout.markerMaxDuration).toBe(s(2));
    // A range marker that started before the view still reaches into it.
    expect(layout.markers[layout.firstMarkerFrom(s(4.5))]?.id).toBe("range");
  });
});

describe("snap guides", () => {
  it("colors each guide by the kind of target it landed on (tokens only)", () => {
    const { view, rec, draw } = setup({
      fps: { num: 24, den: 1 },
      tracks: [track("V")],
      items: [clip("a", "V", 0, 4)],
    });
    view.setViewState({
      snapGuides: [
        { kind: "playhead", t: s(1) },
        { kind: "marker", t: s(2) },
        { kind: "beat", t: s(3) },
        { kind: "in_out", t: s(4) },
        s(5), // a bare time draws as an edge
        { kind: "edge", t: s(6) },
      ],
    });
    draw();
    const strokes = rec.calls.flatMap((c, i) =>
      c.name === "stroke" ? [{ style: c.strokeStyle, dash: rec.calls[i - 1] }] : [],
    );
    const guide = (style: string) => strokes.find((st) => st.style === style);
    for (const [style, dashed] of [
      ["var(--playhead)", false],
      ["var(--marigold-text)", true],
      ["var(--text-muted)", true],
      ["var(--marigold)", false],
      ["var(--text-primary)", true],
    ] as const) {
      const g = guide(style);
      expect(g, style).toBeDefined();
      expect(g?.dash?.name, style).toBe("setLineDash");
      expect(((g?.dash?.args[0] ?? []) as number[]).length > 0, style).toBe(dashed);
    }
  });
});

describe("clips and tracks", () => {
  it("badges linked clips and moves their label past the badge", () => {
    const linked = setup({
      fps: { num: 24, den: 1 },
      tracks: [track("V")],
      items: [clip("a", "V", 0, 4, { linkedGroup: "g1" })],
    });
    const plain = setup({
      fps: { num: 24, den: 1 },
      tracks: [track("V")],
      items: [clip("a", "V", 0, 4)],
    });
    linked.draw();
    plain.draw();
    expect(linked.rec.named("roundRect").length).toBe(plain.rec.named("roundRect").length + 2);
    const labelX = (r: ReturnType<typeof recordingContext>) =>
      r.named("fillText").find((c) => c.args[0] === "clip a")?.args[1] as number;
    expect(labelX(linked.rec) - labelX(plain.rec)).toBe(
      th.space1_5 + th.space2 + th.space0_5 + th.space1,
    );
  });

  it("dims the clips of muted and hidden tracks", () => {
    const bodyAlpha = (extra: Partial<TimelineTrack>) => {
      const { rec, draw } = setup({
        fps: { num: 24, den: 1 },
        tracks: [track("V", extra)],
        items: [clip("a", "V", 0, 4)],
      });
      draw();
      return rec.named("fill").find((c) => c.fillStyle === "var(--clip-media-body)")?.globalAlpha;
    };
    expect(bodyAlpha({})).toBe(1);
    expect(bodyAlpha({ muted: true })).toBe(th.opacityDisabled);
    expect(bodyAlpha({ hidden: true })).toBe(th.opacityDisabled);
  });

  it("hatches locked lanes", () => {
    const hatches = (locked: boolean) => {
      const { rec, draw } = setup({
        fps: { num: 24, den: 1 },
        tracks: [track("V", { locked })],
        items: [clip("a", "V", 0, 4)],
      });
      draw();
      return rec.named("stroke").filter((c) => c.strokeStyle === "var(--border-subtle)").length;
    };
    expect(hatches(false)).toBe(0);
    expect(hatches(true)).toBe(1);
  });

  it("honours the engine's track heights; at track-sm or less a track is collapsed", async () => {
    const model: TimelineModel = {
      fps: { num: 24, den: 1 },
      tracks: [
        track("tall", { height: 120 }),
        track("tiny", { height: 8 }),
        track("lg", { size: "lg" }),
      ],
      items: [
        clip("t", "tall", 0, 4, { thumbnails: { mediaId: "m1", sourceIn: 0 } }),
        clip("c", "tiny", 0, 4, { thumbnails: { mediaId: "m2", sourceIn: 0 } }),
      ],
    };
    const layout = new TimelineLayout(model, {
      sizes: { sm: th.trackSm, md: th.trackMd, lg: th.trackLg },
      pad: th.space1,
    });
    expect(Array.from(layout.trackHeight)).toEqual([
      120 + 2 * th.space1,
      th.trackSm + 2 * th.space1,
      th.trackLg + 2 * th.space1,
    ]);
    // The collapsed track shows labels only: no tiles are asked for it.
    const filmstripTile = vi.fn(async (_req: FilmstripRequest) => null);
    const { draw } = setup(model, { loaders: { filmstripTile } });
    draw();
    await tick();
    const asked = new Set(filmstripTile.mock.calls.map((c) => c[0].mediaId));
    expect(asked).toEqual(new Set(["m1"]));
  });
});

describe("time formats", () => {
  const model: TimelineModel = {
    fps: { num: 24, den: 1 },
    tracks: [track("V")],
    items: [clip("a", "V", 0, 4)],
  };

  it("shows timecode by default and frame numbers when asked", () => {
    const { view, rec, draw } = setup(model);
    draw();
    expect(rec.texts()).toContain("00:00:01:00");
    expect(rec.texts()).toContain("00:00:09:00"); // the playhead flag
    view.setDisplay({ timeFormat: "frames" });
    expect(view.getDisplay().timeFormat).toBe("frames");
    draw();
    expect(rec.texts()).toContain("24");
    expect(rec.texts()).toContain("48");
    expect(rec.texts()).toContain("216"); // the flag: 9 s at 24 fps
    expect(rec.texts()).not.toContain("00:00:01:00");
  });
});

describe("zoom range and smooth zoom", () => {
  const model = generateTimeline({ clipsPerTrack: 20, kinds: ["media", "audio"] });

  it("zooms in to at least 100 px per frame and out to fit 4 hours in a narrow lane", () => {
    for (const fps of [
      { num: 24, den: 1 },
      { num: 60, den: 1 },
      { num: 120, den: 1 },
    ]) {
      const { view } = setup({ fps, tracks: [track("V")], items: [clip("a", "V", 0, 1)] });
      view.zoomTo(1e9);
      expect(view.getViewState().pxPerSec).toBe(MAX_PX_PER_SEC);
      const frame = frameDurationFlicks(fps);
      expect(view.timeToX(frame) - view.timeToX(0)).toBeGreaterThanOrEqual(100);
    }
    const fourHours = s(4 * 3600);
    const { view } = setup(
      { fps: { num: 24, den: 1 }, tracks: [track("V")], items: [clip("a", "V", 0, 4 * 3600)] },
      {},
      400,
      200,
    );
    view.zoomToFit();
    expect(view.timeToX(0)).toBeGreaterThanOrEqual(HEADER);
    expect(view.timeToX(fourHours)).toBeLessThanOrEqual(400);
  });

  it("eases a wheel notch to its exact target, keeping the time under the pointer", () => {
    const { view, frames, advance } = setup(model, { smoothZoom: true });
    view.setViewState({ pxPerSec: 100, scrollX: s(10) });
    frames.run();
    const seen = vi.fn();
    view.onViewChange(seen);
    const anchor = 700;
    const t = view.xToTime(anchor);
    view.zoomSmooth(2, anchor);
    expect(view.zooming).toBe(true);
    const steps: number[] = [];
    for (let i = 0; i < 40 && frames.pending; i++) {
      advance(16);
      frames.run();
      steps.push(view.getViewState().pxPerSec);
      expect(Math.abs(view.xToTime(anchor) - t)).toBeLessThanOrEqual(2);
    }
    expect(view.zooming).toBe(false);
    expect(view.getViewState().pxPerSec).toBe(200);
    expect(steps.length).toBeGreaterThan(2); // eased over several frames, not one jump
    expect(steps[0]).toBeGreaterThan(100);
    expect(steps[0]).toBeLessThan(200);
    // Every step is reported to the host (its viewStore follows), the last one at the target.
    expect(seen.mock.calls.length).toBeGreaterThan(2);
    expect(seen.mock.lastCall?.[0].pxPerSec).toBe(200);
  });

  it("retargets on further notches and applies at once without smooth zoom", () => {
    const { view, frames, advance } = setup(model, { smoothZoom: true });
    view.setViewState({ pxPerSec: 100 });
    frames.run();
    view.zoomSmooth(2, 600);
    advance(16);
    frames.run();
    view.zoomSmooth(2, 600);
    for (let i = 0; i < 60 && frames.pending; i++) {
      advance(16);
      frames.run();
    }
    expect(view.getViewState().pxPerSec).toBe(400);

    const instant = setup(model, { smoothZoom: false });
    instant.view.setViewState({ pxPerSec: 100 });
    instant.view.zoomSmooth(2, 600);
    expect(instant.view.getViewState().pxPerSec).toBe(200);
    expect(instant.view.zooming).toBe(false);
  });

  it("a pan during a smooth zoom carries the zoom along", () => {
    const { view, frames, advance } = setup(model, { smoothZoom: true });
    view.setViewState({ pxPerSec: 100, scrollX: s(10) });
    frames.run();
    const t = view.xToTime(600);
    view.zoomSmooth(4, 600);
    advance(16);
    frames.run();
    const mid = view.getViewState().pxPerSec;
    view.scrollBy(50); // 50 px at the zoom of the moment
    for (let i = 0; i < 60 && frames.pending; i++) {
      advance(16);
      frames.run();
    }
    expect(view.zooming).toBe(false);
    expect(view.getViewState().pxPerSec).toBe(400);
    expect(Math.abs(view.xToTime(600) - (t + (50 * F) / mid))).toBeLessThanOrEqual(2);
  });

  it("zooms to a range with a margin", () => {
    const { view } = setup(model);
    view.zoomToRange(s(20), s(30));
    expect(view.timeToX(s(20))).toBeCloseTo(HEADER + th.space4, 0);
    expect(view.timeToX(s(30))).toBeCloseTo(1200 - th.space4, 0);
  });
});

describe("media never block a frame", () => {
  const bitmap = { width: 96, height: 54, close: () => {} } as unknown as ImageBitmap;
  const model: TimelineModel = {
    fps: { num: 24, den: 1 },
    tracks: [track("V", { size: "lg" }), track("A", { kind: "audio", size: "lg" })],
    items: [
      clip("v", "V", 0, 8, { thumbnails: { mediaId: "m1", sourceIn: 0 } }),
      clip("a", "A", 0, 8, { kind: "audio", waveform: { mediaId: "m1", sourceIn: 0 } }),
    ],
  };
  const peaks = () => ({
    peaksPerSecond: 100,
    min: new Float32Array(1000).fill(-0.5),
    max: new Float32Array(1000).fill(0.5),
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("draws placeholders at once while loads hang", () => {
    const loaders: MediaLoaders = {
      filmstripTile: () => new Promise(() => {}),
      waveform: () => new Promise(() => {}),
    };
    const { view, draw } = setup(model, { loaders });
    draw();
    expect(view.stats.placeholders).toBeGreaterThan(0);
    expect(view.stats.tiles).toBe(0);
  });

  it("shows the nearest cached zoom level while the new one loads, then the new one", async () => {
    const filmstripTile = vi.fn(async () => bitmap);
    const waveform = vi.fn(async () => peaks());
    const { view, rec, draw } = setup(model, { loaders: { filmstripTile, waveform } });
    draw();
    await tick();
    draw();
    expect(view.stats.tiles).toBeGreaterThan(0);
    expect(view.stats.placeholders).toBe(0);

    // One zoom bucket in: nothing of that level is cached yet.
    view.setViewState({ pxPerSec: 100 * 2 ** 0.5 });
    expect(zoomBucket(100 * 2 ** 0.5)).toBe(zoomBucket(100) + 1);
    draw();
    expect(view.stats.fallbackTiles).toBeGreaterThan(0);
    expect(view.stats.placeholders).toBe(0);
    expect(rec.named("stroke").some((c) => c.strokeStyle === "var(--clip-audio)")).toBe(true);
    await tick();
    draw();
    expect(view.stats.fallbackTiles).toBe(0);
    expect(view.stats.tiles).toBeGreaterThan(0);
    expect(waveform).toHaveBeenCalledTimes(2);
  });

  it("asks for nothing on a frame whose tiles are all cached", async () => {
    const filmstripTile = vi.fn(async () => bitmap);
    const { view, draw } = setup(model, { loaders: { filmstripTile } });
    draw();
    await tick();
    draw();
    const request = vi.spyOn(MediaCaches.prototype, "requestTile");
    for (let i = 0; i < 5; i++) draw();
    expect(request).not.toHaveBeenCalled();
    expect(view.stats.tiles).toBeGreaterThan(0);
  });
});

describe("virtualization", () => {
  it("draws no clip body outside the repainted lane area", () => {
    const model = generateTimeline();
    const { view, rec, draw } = setup(model, {}, 1600, 900);
    for (const [pps, scrollSec, scrollY] of [
      [20, 600, 0],
      [4, 3000, 300],
      [0.5, 0, 0],
    ] as const) {
      view.setViewState({ pxPerSec: pps, scrollX: scrollSec * F, scrollY });
      draw();
      const bodies = rec.named("roundRect").filter((c) => (c.args[3] as number) > th.space4); // clip bodies, not the flag
      expect(bodies.length).toBe(view.stats.visibleItems);
      for (const c of bodies) {
        const [bx, by, bw, bh] = c.args as number[];
        expect((bx as number) + (bw as number)).toBeGreaterThanOrEqual(HEADER - 1);
        expect(bx as number).toBeLessThanOrEqual(1600 + 1);
        expect((by as number) + (bh as number)).toBeGreaterThanOrEqual(th.controlMd);
        expect(by as number).toBeLessThanOrEqual(900);
      }
    }
  });
});
