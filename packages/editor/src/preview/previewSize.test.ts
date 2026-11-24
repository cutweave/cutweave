// SPDX-License-Identifier: GPL-3.0-or-later
// Live resize (WP-1M.04, arch §10 step 1): the canvas size in device pixels reaches the engine
// as `view.set_preview_size`, debounced 50 ms, rounded to even, latest wins.
import { describe, expect, it } from "vitest";
import {
  devicePixelSize,
  evenSize,
  PREVIEW_SIZE_DEBOUNCE_MS,
  PreviewSizer,
  type SizerTimers,
} from "./previewSize.ts";

function manualTimers() {
  let t = 0;
  const due: { at: number; cb: () => void; id: number }[] = [];
  let next = 1;
  const timers: SizerTimers = {
    setTimeout: (cb, ms) => {
      const id = next++;
      due.push({ at: t + ms, cb, id });
      return id;
    },
    clearTimeout: (id) => {
      const i = due.findIndex((d) => d.id === id);
      if (i >= 0) due.splice(i, 1);
    },
  };
  return {
    timers,
    advance(ms: number) {
      t += ms;
      for (;;) {
        due.sort((a, b) => a.at - b.at);
        const first = due[0];
        if (first === undefined || first.at > t) break;
        due.shift();
        first.cb();
      }
    },
  };
}

const settle = () => new Promise<void>((r) => setTimeout(r, 0));

describe("evenSize", () => {
  it("rounds each edge to an even number of at least 2", () => {
    expect(evenSize(1280, 720)).toEqual({ width: 1280, height: 720 });
    expect(evenSize(641, 359)).toEqual({ width: 642, height: 360 });
    expect(evenSize(1, 0)).toEqual({ width: 2, height: 2 });
    expect(evenSize(100.6, 50.2)).toEqual({ width: 100, height: 50 });
  });
});

describe("devicePixelSize", () => {
  it("prefers the exact device-pixel box and falls back to CSS size times DPR", () => {
    expect(
      devicePixelSize(
        {
          contentRect: { width: 100, height: 50 },
          devicePixelContentBoxSize: [{ inlineSize: 151, blockSize: 75 }],
        },
        1.5,
      ),
    ).toEqual({ width: 151, height: 75 });
    expect(devicePixelSize({ contentRect: { width: 100.4, height: 50 } }, 1.5)).toEqual({
      width: 151,
      height: 75,
    });
  });
});

describe("PreviewSizer", () => {
  it("sends one request 50 ms after the last change, even-rounded", async () => {
    const sent: [number, number][] = [];
    const m = manualTimers();
    const sizer = new PreviewSizer({
      send: async (w, h) => {
        sent.push([w, h]);
      },
      timers: m.timers,
    });
    sizer.observe(801, 451);
    m.advance(30);
    sizer.observe(901, 501);
    m.advance(PREVIEW_SIZE_DEBOUNCE_MS - 1);
    expect(sent).toEqual([]);
    m.advance(1);
    await settle();
    expect(sent).toEqual([[902, 502]]);
    expect(sizer.requested).toEqual({ width: 902, height: 502 });
  });

  it("skips sizes the engine already has, and empty ones", async () => {
    const sent: [number, number][] = [];
    const m = manualTimers();
    const sizer = new PreviewSizer({
      send: async (w, h) => void sent.push([w, h]),
      timers: m.timers,
    });
    sizer.observe(640, 360);
    m.advance(60);
    await settle();
    sizer.observe(639, 360); // rounds to the same even size
    sizer.observe(0, 360);
    m.advance(60);
    await settle();
    expect(sent).toEqual([[640, 360]]);
  });

  it("latest wins: a change while a request is in flight is sent after it, once", async () => {
    const sent: [number, number][] = [];
    let release: () => void = () => {};
    const m = manualTimers();
    const sizer = new PreviewSizer({
      send: (w, h) => {
        sent.push([w, h]);
        return new Promise<void>((r) => {
          release = r;
        });
      },
      timers: m.timers,
    });
    sizer.observe(400, 300);
    m.advance(60);
    sizer.observe(500, 300);
    m.advance(60);
    sizer.observe(600, 300);
    m.advance(60);
    expect(sent).toEqual([[400, 300]]);
    release();
    await settle();
    expect(sent).toEqual([
      [400, 300],
      [600, 300],
    ]);
  });

  it("resend() repeats the current size (a restarted engine renders at its default)", async () => {
    const sent: [number, number][] = [];
    const m = manualTimers();
    const sizer = new PreviewSizer({
      send: async (w, h) => void sent.push([w, h]),
      timers: m.timers,
    });
    sizer.observe(320, 180);
    m.advance(60);
    await settle();
    sizer.resend();
    await settle();
    expect(sent).toEqual([
      [320, 180],
      [320, 180],
    ]);
  });

  it("a failed request is retried with the next change", async () => {
    const sent: [number, number][] = [];
    let fail = true;
    const m = manualTimers();
    const sizer = new PreviewSizer({
      send: async (w, h) => {
        sent.push([w, h]);
        if (fail) throw new Error("down");
      },
      timers: m.timers,
      onError: () => {},
    });
    sizer.observe(320, 180);
    m.advance(60);
    await settle();
    fail = false;
    sizer.observe(320, 180);
    m.advance(60);
    await settle();
    expect(sent).toEqual([
      [320, 180],
      [320, 180],
    ]);
  });
});
