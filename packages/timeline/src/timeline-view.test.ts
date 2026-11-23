// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from "vitest";
import type { MediaLoaders } from "./cache.ts";
import { generateSmallTimeline, generateTimeline } from "./fixtures.ts";
import { TimelineLayout } from "./layout.ts";
import { fakeCanvas, manualFrames, spyContext, testTheme } from "./test-support.ts";
import { FLICKS_PER_SECOND as F } from "./time.ts";
import { TimelineView } from "./timeline-view.ts";

const th = testTheme();
const tick = (): Promise<void> => new Promise((r) => setTimeout(r, 0));

function setup(options: { loaders?: MediaLoaders } = {}) {
  const spy = spyContext();
  const frames = manualFrames();
  const view = new TimelineView(fakeCanvas(), {
    theme: th,
    context: spy.ctx,
    requestFrame: frames.requestFrame,
    cancelFrame: frames.cancelFrame,
    ...options,
  });
  view.resize(1600, 900, 1);
  return { view, spy, frames };
}

/** Items a correct virtualizer may touch: on a vertically visible lane and overlapping the view. */
function expectedVisible(view: TimelineView, height: number): number {
  const model = view.getModel();
  const layout = new TimelineLayout(model, {
    sizes: { sm: th.trackSm, md: th.trackMd, lg: th.trackLg },
    pad: th.space1,
  });
  const { t0, t1 } = view.visibleRange();
  const top = th.controlMd - view.getViewState().scrollY;
  let n = 0;
  for (let i = 0; i < layout.items.length; i++) {
    const ti = layout.trackOf[i] as number;
    const laneTop = top + (layout.trackTop[ti] as number);
    if (laneTop + (layout.trackHeight[ti] as number) <= th.controlMd || laneTop >= height) continue;
    if ((layout.start[i] as number) <= t1 + 1 && (layout.end[i] as number) >= t0 - 1) n++;
  }
  return n;
}

describe("virtualization (5,000-item fixture)", () => {
  const model = generateTimeline();

  it("has the S1 stress shape", () => {
    expect(model.items).toHaveLength(5000);
    expect(model.items.reduce((n, i) => n + (i.keyframes?.length ?? 0), 0)).toBe(50_000);
  });

  it.each([
    ["zoomed in", 20, 600],
    ["mid zoom", 4, 3000],
    ["zoomed way out (whole project)", 0.4, 0],
    ["scrolled down", 20, 1500, 300],
  ])("draws at most visible + 2 clip rects: %s", (_n, pps, scrollSec, scrollY = 0) => {
    const { view, spy } = setup();
    view.setModel(model);
    view.setViewState({ pxPerSec: pps, scrollX: scrollSec * F, scrollY });
    spy.reset();
    expect(view.flush()).toBe(true);

    const visible = expectedVisible(view, 900);
    expect(view.stats.visibleItems).toBe(visible);
    // One roundRect per clip body, plus the playhead flag; nothing for offscreen clips.
    expect(spy.calls.roundRect ?? 0).toBeLessThanOrEqual(visible + 2);
    expect(spy.calls.roundRect ?? 0).toBeGreaterThanOrEqual(visible);
    expect(visible).toBeLessThan(5000);
  });

  it("culls far below the item count when zoomed in", () => {
    const { view, spy } = setup();
    view.setModel(model);
    view.setViewState({ pxPerSec: 20, scrollX: 600 * F });
    spy.reset();
    view.flush();
    expect(spy.calls.roundRect ?? 0).toBeLessThan(500);
  });

  it("batches by track: far fewer fill/stroke calls than clips", () => {
    const { view, spy } = setup();
    view.setModel(model);
    view.setViewState({ pxPerSec: 4, scrollX: 3000 * F });
    spy.reset();
    view.flush();
    const clips = view.stats.visibleItems;
    expect(clips).toBeGreaterThan(300);
    expect((spy.calls.fill ?? 0) + (spy.calls.stroke ?? 0)).toBeLessThan(clips / 2);
  });

  it("caps labels per frame and hides labels, keyframes on tiny clips", () => {
    const { view } = setup();
    view.setModel(model);
    view.setViewState({ pxPerSec: 4, scrollX: 3000 * F });
    view.flush();
    expect(view.stats.labels).toBeLessThanOrEqual(400);
    expect(view.stats.labels).toBeGreaterThan(0);

    view.setViewState({ pxPerSec: 0.4, scrollX: 0 });
    view.flush();
    expect(view.stats.labels).toBe(0);
    expect(view.stats.visibleKeyframes).toBe(0);

    view.setViewState({ pxPerSec: 60, scrollX: 3000 * F });
    view.flush();
    expect(view.stats.visibleKeyframes).toBeGreaterThan(0);
  });
});

describe("render loop and dirty regions", () => {
  it("coalesces changes into one frame and draws nothing when clean", () => {
    const { view, frames, spy } = setup();
    view.setModel(generateSmallTimeline());
    expect(frames.pending).toBe(true);
    expect(frames.run()).toBe(true);
    expect(frames.pending).toBe(false);
    spy.reset();
    expect(view.flush()).toBe(false);
    expect(spy.calls.fillRect ?? 0).toBe(0);
    view.setViewState({ pxPerSec: 150 });
    view.setViewState({ pxPerSec: 160 });
    view.setViewState({ playhead: 5 * F });
    expect(frames.pending).toBe(true);
    expect(frames.run()).toBe(true);
    expect(frames.run()).toBe(false);
  });

  it("repaints only a strip when just the playhead moves", () => {
    const { view, spy } = setup();
    view.setModel(generateTimeline());
    view.setViewState({ pxPerSec: 20, scrollX: 600 * F, playhead: 610 * F });
    spy.reset();
    view.flush();
    const full = spy.calls.roundRect ?? 0;
    expect(full).toBeGreaterThan(50);

    view.setViewState({ playhead: 611 * F });
    spy.reset();
    expect(view.flush()).toBe(true);
    const strip = spy.calls.roundRect ?? 0;
    expect(strip).toBeLessThan(full / 3);
    expect(spy.calls.clip ?? 0).toBeGreaterThan(0);
  });

  it("repaints everything on scroll, zoom, selection and model changes", () => {
    const { view, spy } = setup();
    view.setModel(generateSmallTimeline());
    view.flush();
    spy.reset();
    view.flush();
    const idle = spy.calls.roundRect ?? 0;
    for (const patch of [
      { scrollX: 2 * F },
      { pxPerSec: 300 },
      { selection: new Set(["itm_1"]) },
      { snapGuides: [F] },
    ]) {
      view.setViewState(patch);
      spy.reset();
      expect(view.flush()).toBe(true);
      expect(spy.calls.roundRect ?? 0).toBeGreaterThan(idle);
    }
    view.setModel(generateSmallTimeline());
    expect(view.flush()).toBe(true);
  });

  it("draws selection outline, proposed hatch, snap guides and a marquee when asked", () => {
    const { view, spy } = setup();
    view.setModel(generateSmallTimeline());
    view.setViewState({
      pxPerSec: 80,
      selection: new Set(["itm_1"]),
      snapGuides: [3 * F],
      marquee: { x: 300, y: 40, w: 100, h: 60 },
      range: { in: F, out: 4 * F },
    });
    spy.reset();
    view.flush();
    expect(spy.calls.setLineDash ?? 0).toBeGreaterThanOrEqual(4); // dashed proposal + snap guide
    expect(spy.calls.strokeRect).toBe(1); // marquee
    expect(spy.calls.clip ?? 0).toBeGreaterThanOrEqual(2); // frame clip + proposal hatch
  });
});

describe("ruler labels and the playhead flag (issue #97)", () => {
  /** A no-op context that records the text it draws (6 px per character). */
  function textContext(): { ctx: CanvasRenderingContext2D; texts: string[] } {
    const texts: string[] = [];
    const ctx = new Proxy(
      {},
      {
        get(_t, prop) {
          if (prop === "measureText") return (s: string) => ({ width: s.length * 6 });
          if (prop === "fillText") return (s: string) => texts.push(s);
          return () => undefined;
        },
        set: () => true,
      },
    ) as CanvasRenderingContext2D;
    return { ctx, texts };
  }

  function rulerSetup() {
    const { ctx, texts } = textContext();
    const frames = manualFrames();
    const view = new TimelineView(fakeCanvas(), {
      theme: th,
      context: ctx,
      requestFrame: frames.requestFrame,
      cancelFrame: frames.cancelFrame,
    });
    view.resize(1600, 300, 1);
    view.setModel(generateSmallTimeline());
    // 100 px/s: ruler labels every second, 100 px apart; the flag is about 80 px wide.
    view.setViewState({ pxPerSec: 100, scrollX: 0, playhead: 8 * F });
    return { view, texts };
  }

  it("hides the ruler labels the flag would cover", () => {
    const { view, texts } = rulerSetup();
    view.flush();
    expect(texts.filter((s) => s === "00:00:08:00")).toHaveLength(1); // the flag itself
    expect(texts).not.toContain("00:00:07:00"); // runs into the flag from the left
    expect(texts).toContain("00:00:06:00");
    expect(texts).toContain("00:00:09:00");
  });

  it("redraws a label whole once the flag moves off it", () => {
    const { view, texts } = rulerSetup();
    view.flush();
    texts.length = 0;
    view.setViewState({ playhead: 8.5 * F });
    expect(view.flush()).toBe(true);
    expect(texts).toContain("00:00:07:00"); // inside the repainted strip
    expect(texts).not.toContain("00:00:08:00"); // now under the flag
    expect(texts.some((s) => s.startsWith("00:00:08:") && s !== "00:00:08:00")).toBe(true); // flag
  });
});

describe("pluggable media", () => {
  const bitmap = { width: 96, height: 54, close: vi.fn() } as unknown as ImageBitmap;

  it("shows placeholders, requests tiles through the loader, then draws them", async () => {
    const filmstripTile = vi.fn(async () => bitmap);
    const { view, frames } = setup({ loaders: { filmstripTile } });
    view.setModel(generateSmallTimeline());
    view.setViewState({ pxPerSec: 100 });
    view.flush();
    expect(view.stats.tiles).toBe(0);
    await tick();
    expect(filmstripTile).toHaveBeenCalled();
    const requested = filmstripTile.mock.calls.length;
    expect(view.cacheStats.tiles).toBe(requested);
    expect(frames.pending).toBe(true); // onLoaded invalidated the view
    view.flush();
    expect(view.stats.tiles).toBeGreaterThan(0);
    // Each distinct tile loads once: redrawing does not re-request.
    view.invalidate();
    view.flush();
    expect(filmstripTile.mock.calls.length).toBe(requested);
  });

  it("requests waveform peaks once per media and zoom bucket", async () => {
    const waveform = vi.fn(async () => ({
      peaksPerSecond: 200,
      min: new Float32Array(4000).fill(-0.5),
      max: new Float32Array(4000).fill(0.5),
    }));
    const { view } = setup({ loaders: { waveform } });
    view.setModel(generateSmallTimeline());
    view.flush();
    await tick();
    view.flush();
    view.flush();
    expect(waveform).toHaveBeenCalledTimes(1);
    expect(view.cacheStats.peaks).toBe(1);
  });

  it("makes no requests without loaders", () => {
    const { view } = setup();
    view.setModel(generateSmallTimeline());
    view.flush();
    expect(view.cacheStats.pending).toBe(0);
  });
});

describe("view API for interactions", () => {
  it("maps time and x both ways and exposes item rects", () => {
    const { view } = setup();
    view.setModel(generateSmallTimeline());
    view.setViewState({ pxPerSec: 100 });
    const r = view.itemRect("itm_1");
    expect(r).not.toBeNull();
    expect(view.timeToX(Math.round(0.5 * F))).toBeCloseTo(r?.x ?? 0, 6);
    expect(view.xToTime(view.timeToX(12 * F))).toBe(12 * F);
    expect(view.itemRect("nope")).toBeNull();
  });

  it("hit-tests through the layout", () => {
    const { view } = setup();
    view.setModel(generateSmallTimeline());
    view.setViewState({ pxPerSec: 100 });
    const r = view.itemRect("itm_1");
    if (!r) throw new Error("itemRect");
    expect(view.hitTest(r.x + r.w / 2, r.y + r.h / 2)).toMatchObject({
      zone: "body",
      itemId: "itm_1",
    });
    expect(view.hitTest(r.x + 2, r.y + r.h / 2).zone).toBe("left-edge");
    expect(view.hitTest(r.x + r.w - 2, r.y + r.h / 2).zone).toBe("right-edge");
    expect(view.hitTest(300, 5).zone).toBe("ruler");
  });

  it("finds items for a marquee rectangle", () => {
    const { view } = setup();
    view.setModel(generateSmallTimeline());
    view.setViewState({ pxPerSec: 100 });
    const a = view.itemRect("itm_1");
    const b = view.itemRect("itm_4");
    if (!a || !b) throw new Error("itemRect");
    const ids = view.itemsInRect({ x: a.x + 5, y: a.y + 5, w: 40, h: b.y - a.y + 10 });
    expect(ids.sort()).toEqual(["itm_1", "itm_4"]);
    expect(view.itemsInRect({ x: 0, y: 0, w: 5, h: 5 })).toEqual([]);
  });

  it("zooms around an anchor and reports user changes, but not host-applied ones", () => {
    const { view } = setup();
    view.setModel(generateTimeline({ clipsPerTrack: 20 }));
    const seen = vi.fn();
    view.onViewChange(seen);
    view.setViewState({ pxPerSec: 50, scrollX: 10 * F });
    expect(seen).not.toHaveBeenCalled();
    const anchor = 800;
    const t = view.xToTime(anchor);
    view.zoomBy(2, anchor);
    expect(seen).toHaveBeenCalledTimes(1);
    expect(view.getViewState().pxPerSec).toBe(100);
    expect(Math.abs(view.xToTime(anchor) - t)).toBeLessThanOrEqual(1);
    const before = view.getViewState().scrollX;
    view.scrollBy(200); // 200 px at 100 px/s = 2 s
    expect(view.getViewState().scrollX - before).toBe(2 * F);
    expect(seen).toHaveBeenCalledTimes(2);
  });

  it("clamps scroll to the content and zoom to its limits", () => {
    const { view } = setup();
    view.setModel(generateSmallTimeline());
    view.setViewState({ scrollX: -100 * F, scrollY: 5000, pxPerSec: 1e9 });
    const s = view.getViewState();
    expect(s.scrollX).toBe(0);
    expect(s.scrollY).toBe(0);
    expect(s.pxPerSec).toBe(20_000);
  });

  it("fits the project into the lane", () => {
    const { view } = setup();
    const m = generateSmallTimeline();
    view.setModel(m);
    view.zoomToFit();
    const end = Math.max(...m.items.map((i) => i.start + i.duration));
    expect(view.timeToX(end)).toBeLessThanOrEqual(1600);
    expect(view.timeToX(end)).toBeGreaterThan(1200);
  });
});
