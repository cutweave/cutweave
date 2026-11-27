// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-2A.13: what the Timeline preferences change. Each display row changes what the renderer
// draws (one check per value): how time shows, drop-frame timecode, video clip thumbnails, audio
// waveforms and the height of a track. The mouse wheel scrolls or zooms; every follow mode keeps
// the playhead in sight while playing, except Off.
import { describe, expect, it, vi } from "vitest";
import type { FollowMode, TimelineItem, TimelineModel, TimelineTrack, TrackSize } from "./model.ts";
import { fakeCanvas, manualFrames, recordingContext, testTheme } from "./test-support.ts";
import { FLICKS_PER_SECOND as F, formatSeconds, formatTime, frameDurationFlicks } from "./time.ts";
import { TimelineView, type TimelineViewOptions } from "./timeline-view.ts";
import { followScrollX } from "./viewport.ts";

const th = testTheme();
const s = (sec: number): number => Math.round(sec * F);
const tick = (): Promise<void> => new Promise((r) => setTimeout(r, 0));

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

function setup(
  model: TimelineModel,
  opts: TimelineViewOptions = {},
  canvas: HTMLCanvasElement = fakeCanvas(),
) {
  const rec = recordingContext();
  const frames = manualFrames();
  const view = new TimelineView(canvas, {
    theme: th,
    context: rec.ctx,
    requestFrame: frames.requestFrame,
    cancelFrame: frames.cancelFrame,
    smoothZoom: false,
    ...opts,
  });
  view.resize(1200, 400, 1);
  view.setModel(model);
  view.setViewState({ pxPerSec: 100, scrollX: 0, playhead: s(9) });
  const draw = () => {
    rec.calls.length = 0;
    view.invalidate();
    view.flush();
  };
  return { view, rec, frames, draw };
}

describe("Show time as", () => {
  const model: TimelineModel = {
    fps: { num: 24, den: 1 },
    tracks: [track("V")],
    items: [clip("a", "V", 0, 4)],
  };

  it("draws the ruler and the playhead flag as timecode, frames or seconds", () => {
    const { view, rec, draw } = setup(model);
    const drawn: Record<string, string[]> = {};
    for (const timeFormat of ["timecode", "frames", "seconds"] as const) {
      view.setDisplay({ timeFormat });
      draw();
      drawn[timeFormat] = rec.texts();
    }
    expect(drawn.timecode).toContain("00:00:01:00");
    expect(drawn.timecode).toContain("00:00:09:00"); // the playhead flag
    expect(drawn.frames).toContain("24");
    expect(drawn.frames).toContain("216");
    expect(drawn.frames).not.toContain("00:00:01:00");
    expect(drawn.seconds).toContain("1s");
    expect(drawn.seconds).toContain("9s");
    expect(drawn.seconds).not.toContain("24");
    expect(drawn.seconds).not.toContain("00:00:01:00");
  });

  it("formats one time in each form", () => {
    const fps = { num: 24, den: 1 };
    expect(formatTime(s(2.5), fps, "timecode")).toBe("00:00:02:12");
    expect(formatTime(s(2.5), fps, "frames")).toBe("60");
    expect(formatTime(s(2.5), fps, "seconds")).toBe("2.5s");
    expect(formatSeconds(s(12))).toBe("12s");
    expect(formatSeconds(s(0.04))).toBe("0.04s");
    expect(formatSeconds(-5)).toBe("0s");
  });
});

describe("Drop-frame timecode", () => {
  const model: TimelineModel = {
    fps: { num: 30000, den: 1001 },
    tracks: [track("V")],
    items: [clip("a", "V", 0, 4)],
  };

  it("shows `;` before the frames at 29.97 when on, `:` when off", () => {
    const { view, rec, draw } = setup(model);
    view.setViewState({ playhead: s(61) });
    view.setDisplay({ dropFrame: true });
    draw();
    const on = rec.texts();
    expect(on.some((text) => /^\d\d:\d\d:\d\d;\d\d$/.test(text))).toBe(true);
    expect(on.some((text) => /^\d\d:\d\d:\d\d:\d\d$/.test(text))).toBe(false);
    view.setDisplay({ dropFrame: false });
    draw();
    const off = rec.texts();
    expect(off.some((text) => /^\d\d:\d\d:\d\d;\d\d$/.test(text))).toBe(false);
    expect(off.some((text) => /^\d\d:\d\d:\d\d:\d\d$/.test(text))).toBe(true);
    // A minute in, the two disagree about the frame: drop-frame skips two labels a minute.
    expect(formatTime(s(61), model.fps, "timecode", true)).not.toBe(
      formatTime(s(61), model.fps, "timecode", false).replace(/:(\d\d)$/, ";$1"),
    );
  });

  it("never applies at a rate that has no drop-frame timecode", () => {
    const { view, rec, draw } = setup({ ...model, fps: { num: 25, den: 1 } });
    view.setDisplay({ dropFrame: true });
    draw();
    expect(rec.texts().some((text) => text.includes(";"))).toBe(false);
  });
});

describe("Video clip thumbnails", () => {
  const bitmap = { width: 96, height: 54, close: vi.fn() } as unknown as ImageBitmap;
  const model: TimelineModel = {
    fps: { num: 24, den: 1 },
    tracks: [track("V", { size: "lg" })],
    items: [clip("a", "V", 0, 10, { thumbnails: { mediaId: "m1", sourceIn: 0 } })],
  };

  async function tilesDrawn(thumbnails: "filmstrip" | "first" | "off") {
    const filmstripTile = vi.fn(async () => bitmap);
    const { view, rec, draw } = setup(model, { loaders: { filmstripTile } });
    view.setDisplay({ thumbnails });
    draw();
    await tick();
    draw();
    return {
      requested: filmstripTile.mock.calls.length,
      tiles: view.stats.tiles,
      images: rec.named("drawImage"),
    };
  }

  it("filmstrip: tiles all along the clip", async () => {
    const strip = await tilesDrawn("filmstrip");
    expect(strip.tiles).toBeGreaterThan(3);
    expect(strip.images.length).toBe(strip.tiles);
  });

  it("first frame: one tile, at the clip's left edge", async () => {
    const first = await tilesDrawn("first");
    expect(first.requested).toBe(1);
    expect(first.tiles).toBe(1);
    expect(first.images).toHaveLength(1);
    // Drawn from the clip's start (x of time 0 is the header's right edge).
    const view = setup(model).view;
    const args = first.images[0]?.args as number[];
    expect(args[args.length - 4]).toBeCloseTo(view.timeToX(0), 5);
  });

  it("off: no tile is asked for or drawn", async () => {
    const off = await tilesDrawn("off");
    expect(off.requested).toBe(0);
    expect(off.tiles).toBe(0);
    expect(off.images).toHaveLength(0);
  });
});

describe("Audio waveforms on clips", () => {
  const model: TimelineModel = {
    fps: { num: 24, den: 1 },
    tracks: [track("A", { kind: "audio", size: "lg" })],
    items: [clip("a", "A", 0, 10, { kind: "audio", waveform: { mediaId: "m1", sourceIn: 0 } })],
  };
  const peaks = () => ({
    peaksPerSecond: 200,
    min: new Float32Array(4000).fill(-0.5),
    max: new Float32Array(4000).fill(0.5),
  });

  async function lines(waveforms: boolean) {
    const waveform = vi.fn(async () => peaks());
    const { view, rec, draw } = setup(model, { loaders: { waveform } });
    view.setDisplay({ waveforms });
    draw();
    await tick();
    draw();
    return { asked: waveform.mock.calls.length, lineTo: rec.named("lineTo").length };
  }

  it("on: the peaks are loaded and drawn; off: neither", async () => {
    const on = await lines(true);
    const off = await lines(false);
    expect(on.asked).toBe(1);
    expect(off.asked).toBe(0);
    expect(on.lineTo).toBeGreaterThan(off.lineTo + 100);
  });
});

describe("Height of a track", () => {
  it("draws small, medium and large tracks at their token heights", () => {
    const heights: Record<string, number> = {};
    for (const [size, px] of [
      ["sm", th.trackSm],
      ["md", th.trackMd],
      ["lg", th.trackLg],
    ] as const satisfies readonly (readonly [TrackSize, number])[]) {
      // The engine stores the height in pixels (`track.set_height`): what the editor sends for
      // "Height of new tracks".
      const { view } = setup({
        fps: { num: 24, den: 1 },
        tracks: [track("V", { height: px })],
        items: [clip("a", "V", 0, 4)],
      });
      heights[size] = view.itemRect("a")?.h ?? 0;
      expect(heights[size]).toBe(px);
    }
    expect(heights.sm).toBeLessThan(heights.md as number);
    expect(heights.md).toBeLessThan(heights.lg as number);
  });
});

describe("Mouse wheel", () => {
  const model: TimelineModel = {
    fps: { num: 24, den: 1 },
    tracks: [track("V")],
    items: [clip("a", "V", 0, 600)],
  };

  function bound() {
    const box = document.createElement("div");
    const canvas = document.createElement("canvas");
    box.append(canvas);
    document.body.append(box);
    const made = setup(model, {}, canvas);
    made.view.bind();
    made.view.resize(1200, 400, 1);
    const wheel = (init: WheelEventInit): void => {
      canvas.dispatchEvent(new WheelEvent("wheel", { deltaY: 120, cancelable: true, ...init }));
    };
    return { ...made, wheel };
  }

  it("scrolls, and zooms with Ctrl, by default", () => {
    const { view, wheel } = bound();
    const before = view.getViewState();
    wheel({});
    expect(view.getViewState().scrollX).toBeGreaterThan(before.scrollX);
    expect(view.getViewState().pxPerSec).toBe(before.pxPerSec);
    wheel({ ctrlKey: true });
    expect(view.getViewState().pxPerSec).toBeLessThan(before.pxPerSec);
    view.destroy();
  });

  it("zooms, and scrolls with Ctrl, when set to zoom", () => {
    const { view, wheel } = bound();
    view.setDisplay({ wheel: "zoom" });
    const before = view.getViewState();
    wheel({});
    const zoomed = view.getViewState();
    expect(zoomed.pxPerSec).toBeLessThan(before.pxPerSec);
    wheel({ ctrlKey: true });
    expect(view.getViewState().pxPerSec).toBe(zoomed.pxPerSec);
    expect(view.getViewState().scrollX).toBeGreaterThan(zoomed.scrollX);
    // Shift+wheel scrolls along the time in both settings.
    const at = view.getViewState();
    wheel({ shiftKey: true });
    expect(view.getViewState().pxPerSec).toBe(at.pxPerSec);
    expect(view.getViewState().scrollX).toBeGreaterThan(at.scrollX);
    view.destroy();
  });
});

describe("Follow the playhead while playing", () => {
  const fps = { num: 24, den: 1 };
  const model: TimelineModel = {
    fps,
    tracks: [track("V")],
    items: [clip("a", "V", 0, 120)],
  };

  /** Plays 60 s a frame at a time; returns where the view was at each frame. */
  function play(mode: FollowMode) {
    const { view } = setup(model);
    view.setViewState({ pxPerSec: 100, scrollX: 0, playhead: 0 });
    const frame = frameDurationFlicks(fps);
    const scrolls: number[] = [];
    let seen = true;
    for (let t = 0; t <= s(60); t += frame) {
      view.setViewState({ playhead: t });
      const { t0, t1 } = view.visibleRange();
      const next = followScrollX(mode, t0, t1, t);
      if (next !== t0) view.setViewState({ scrollX: next });
      const now = view.visibleRange();
      seen &&= t >= now.t0 && t <= now.t1;
      scrolls.push(view.getViewState().scrollX);
    }
    return { scrolls, seen, moves: new Set(scrolls).size - 1 };
  }

  it("page: the view turns a page when the playhead reaches its edge", () => {
    const page = play("page");
    expect(page.seen).toBe(true);
    // A few pages in a minute at 100 px/s, never a move per frame.
    expect(page.moves).toBeGreaterThan(2);
    expect(page.moves).toBeLessThan(20);
  });

  it("smooth: the playhead stays in the middle and the timeline moves under it", () => {
    const smooth = play("smooth");
    expect(smooth.seen).toBe(true);
    expect(smooth.moves).toBeGreaterThan(1000);
  });

  it("off: the view stays where it was and the playhead leaves it", () => {
    const off = play("off");
    expect(off.moves).toBe(0);
    expect(off.seen).toBe(false);
  });

  it("brings a playhead that is out of sight back, and never scrolls before zero", () => {
    expect(followScrollX("page", s(100), s(110), s(5))).toBe(s(4.5));
    expect(followScrollX("smooth", s(100), s(110), s(2))).toBe(0);
    expect(followScrollX("off", s(100), s(110), s(5))).toBe(s(100));
    expect(followScrollX("page", s(0), s(10), s(3))).toBe(0);
  });
});
