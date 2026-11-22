// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from "vitest";
import type { PreviewFrame } from "../shared/api.ts";
import {
  currentFramePresenter,
  DEFAULT_LATENCY_MS,
  type FramePresenter,
  type LoopClock,
  PresentLoop,
  setFramePresenter,
} from "./present-loop.ts";

function frame(seq: number): PreviewFrame {
  return {
    pts: seq * 11_760_000,
    duration: 11_760_000,
    seq,
    width: 4,
    height: 2,
    format: "rgba8",
    bytes: new ArrayBuffer(32),
    stride: 16,
    plane1Offset: 0,
    plane1Stride: 0,
    lowres: false,
    rev: 1,
    configGen: 1,
    renderAgeMs: 0,
    publishAgeMs: 0,
  };
}

/** A manual clock: `step()` runs the scheduled tick, advancing time by 16.67 ms. */
function manualClock() {
  let pending: (() => void) | null = null;
  let t = 0;
  const clock: LoopClock = {
    schedule: (tick) => {
      pending = tick;
      return 1;
    },
    cancel: () => {
      pending = null;
    },
    now: () => t,
  };
  return {
    clock,
    step() {
      t += 1000 / 60;
      const tick = pending;
      pending = null;
      tick?.();
    },
    get scheduled() {
      return pending !== null;
    },
  };
}

describe("PresentLoop", () => {
  it("asks the addon for the clock-scheduled frame with the default latency and presents it", () => {
    const frames = [frame(1), null, frame(2)];
    const source = { frameAtClock: vi.fn(() => frames.shift() ?? null) };
    const presented: number[] = [];
    const presenter: FramePresenter = { present: (f) => presented.push(f.seq) };
    const m = manualClock();
    const loop = new PresentLoop(source, presenter, m.clock);
    loop.start();
    for (let i = 0; i < 3; i++) m.step();
    expect(source.frameAtClock).toHaveBeenCalledWith(DEFAULT_LATENCY_MS);
    expect(presented).toEqual([1, 2]);
    const stats = loop.stats();
    expect(stats).toMatchObject({
      running: true,
      framesPresented: 2,
      seqGaps: 0,
      lastSeq: 2,
      lastWidth: 4,
      lastFormat: "rgba8",
      errors: 0,
    });
    expect(stats.presentFps).toBe(2);
  });

  it("counts gaps in seq, honours a custom latency, and keeps no pixels in the stats", () => {
    const seqs = [1, 2, 5];
    const source = { frameAtClock: vi.fn(() => frame(seqs.shift() ?? 99)) };
    const m = manualClock();
    const loop = new PresentLoop(source, null, m.clock, { latencyMs: 33 });
    loop.start();
    for (let i = 0; i < 3; i++) m.step();
    expect(source.frameAtClock).toHaveBeenCalledWith(33);
    expect(loop.stats().seqGaps).toBe(2);
    expect(JSON.stringify(loop.stats())).not.toContain("bytes");
  });

  it("stops on an error and reports it", () => {
    const source = {
      frameAtClock: vi.fn(() => {
        throw new Error("ERR_RING_CLOSED: no preview ring is open");
      }),
    };
    const m = manualClock();
    const loop = new PresentLoop(source, null, m.clock);
    loop.start();
    m.step();
    expect(m.scheduled).toBe(false);
    expect(loop.stats()).toMatchObject({
      running: false,
      errors: 1,
      lastError: "ERR_RING_CLOSED: no preview ring is open",
    });
  });

  it("stop() cancels the pending tick; start() twice schedules once", () => {
    const source = { frameAtClock: vi.fn(() => null) };
    const m = manualClock();
    const loop = new PresentLoop(source, null, m.clock);
    loop.start();
    loop.start();
    loop.stop();
    m.step();
    expect(source.frameAtClock).not.toHaveBeenCalled();
    expect(loop.running).toBe(false);
  });

  it("WP-1M.04 installs its presenter once, for every loop started afterwards", () => {
    const p: FramePresenter = { present: () => {} };
    setFramePresenter(p);
    expect(currentFramePresenter()).toBe(p);
    setFramePresenter(null);
    expect(currentFramePresenter()).toBeNull();
  });
});
