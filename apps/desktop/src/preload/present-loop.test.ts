// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from "vitest";
import type { PreviewFrame } from "../shared/api.ts";
import {
  currentFramePresenter,
  DEFAULT_LATENCY_MS,
  type FramePresenter,
  installedPresenter,
  type LoopClock,
  canvasLoopClock,
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
    /** Time passes without a tick (a stall). */
    advance(ms: number) {
      t += ms;
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

  it("counts display ticks, ticks per second and late ticks (presenter instrumentation)", () => {
    const source = { frameAtClock: vi.fn(() => null) };
    const m = manualClock();
    const loop = new PresentLoop(source, null, m.clock);
    loop.start();
    for (let i = 0; i < 90; i++) m.step();
    expect(loop.stats()).toMatchObject({ ticks: 90, longTicks: 0, presenter: "none" });
    expect(loop.stats().tickFps).toBeGreaterThanOrEqual(60);
    expect(loop.stats().tickFps).toBeLessThanOrEqual(61);
    // A stall: the next tick comes 50 ms late.
    m.step();
    expect(loop.stats().longTicks).toBe(0);
    m.advance(50);
    m.step();
    expect(loop.stats().longTicks).toBe(1);
  });

  it("reports the installed presenter's state through the forwarding presenter", () => {
    const presented: number[] = [];
    setFramePresenter({
      present: (f) => presented.push(f.seq),
      stats: () => ({ kind: "webgpu", errors: 2, lost: true, lastError: "device lost" }),
    });
    const source = { frameAtClock: vi.fn(() => frame(1)) };
    const m = manualClock();
    const loop = new PresentLoop(source, installedPresenter, m.clock);
    loop.start();
    m.step();
    expect(presented).toEqual([1]);
    expect(loop.stats()).toMatchObject({
      presenter: "webgpu",
      presenterErrors: 2,
      presenterLost: true,
      presenterError: "device lost",
    });
    setFramePresenter(null);
    expect(loop.stats().presenter).toBe("none");
  });

  it("WP-1M.04 installs its presenter once, for every loop started afterwards", () => {
    const p: FramePresenter = { present: () => {} };
    setFramePresenter(p);
    expect(currentFramePresenter()).toBe(p);
    setFramePresenter(null);
    expect(currentFramePresenter()).toBeNull();
  });
});

/** A window with its own frames: `frame()` runs what it was asked for, like one display frame. */
function fakeWindow() {
  let next = 1;
  const asked = new Map<number, FrameRequestCallback>();
  const pagehide: (() => void)[] = [];
  const win = {
    closed: false,
    requestAnimationFrame: vi.fn((cb: FrameRequestCallback) => {
      asked.set(next, cb);
      return next++;
    }),
    cancelAnimationFrame: vi.fn((id: number) => {
      asked.delete(id);
    }),
    addEventListener: (_type: "pagehide", listener: () => void) => {
      pagehide.push(listener);
    },
    frame() {
      const due = [...asked.values()];
      asked.clear();
      for (const cb of due) cb(0);
    },
    close() {
      win.closed = true;
      for (const listener of pagehide) listener();
      asked.clear();
    },
  };
  return win;
}

// WP-2H.15: a Preview detached into its own window is presented on that window's frames.
describe("the canvas clock", () => {
  it("ticks on the frames of the window the canvas is in, read at every tick", () => {
    const main = fakeWindow();
    const popout = fakeWindow();
    let canvasIn: ReturnType<typeof fakeWindow> | null = null;
    const source = { frameAtClock: vi.fn(() => frame(1)) };
    const loop = new PresentLoop(
      source,
      null,
      canvasLoopClock(() => canvasIn, main, () => 0),
    );
    loop.start();
    expect(main.requestAnimationFrame).toHaveBeenCalledTimes(1);
    // The canvas moves into the detached window: its next tick is that window's.
    canvasIn = popout;
    main.frame();
    expect(popout.requestAnimationFrame).toHaveBeenCalledTimes(1);
    popout.frame();
    popout.frame();
    expect(popout.requestAnimationFrame).toHaveBeenCalledTimes(3);
    expect(main.requestAnimationFrame).toHaveBeenCalledTimes(1);
    expect(loop.stats().ticks).toBe(3);
    // Stopping cancels the frame in the window that owes it.
    loop.stop();
    expect(popout.cancelAnimationFrame).toHaveBeenCalledTimes(1);
  });

  it("asks the main window again when the canvas's window closes before its frame comes", () => {
    const main = fakeWindow();
    const popout = fakeWindow();
    let canvasIn: ReturnType<typeof fakeWindow> | null = popout;
    const loop = new PresentLoop(
      { frameAtClock: () => null },
      null,
      canvasLoopClock(() => canvasIn, main, () => 0),
    );
    loop.start();
    expect(popout.requestAnimationFrame).toHaveBeenCalledTimes(1);
    // The person closes the detached window: the dock docks the Preview back.
    canvasIn = null;
    popout.close();
    expect(main.requestAnimationFrame).toHaveBeenCalledTimes(1);
    main.frame();
    expect(loop.stats().ticks).toBe(1);
    expect(loop.running).toBe(true);
    // A closed window is never asked again.
    canvasIn = popout;
    main.frame();
    expect(popout.requestAnimationFrame).toHaveBeenCalledTimes(1);
    loop.stop();
  });
});
