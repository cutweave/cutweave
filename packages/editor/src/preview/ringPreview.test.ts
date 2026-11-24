// SPDX-License-Identifier: GPL-3.0-or-later
// The GPU path's page side (WP-1M.04): attach the canvas to the preload's WebGPU presenter,
// open the ring the engine announced, follow it when it ends, and fall back with a reason.
import { describe, expect, it, vi } from "vitest";
import {
  type PreviewBridge,
  parseRingAnnouncement,
  RingPreview,
  type RingPreviewState,
  type WatchTimers,
} from "./ringPreview.ts";

const RING = { name: "cwp-00001234-00000000000000aa", epoch: "0x00000000000000ff" };

function fakeBridge(over: Partial<PreviewBridge> = {}) {
  const calls: string[] = [];
  let status: { state: "live" | "starting" | "ended"; reason?: string } = { state: "live" };
  let running = true;
  const bridge: PreviewBridge = {
    isAvailable: () => true,
    attachCanvas: vi.fn(async () => {
      calls.push("attach");
      return { ok: true as const, adapter: "Fake GPU", upload: "writeTexture" };
    }),
    detachCanvas: () => void calls.push("detach"),
    redraw: () => {
      calls.push("redraw");
      return true;
    },
    open: (name, epoch) => {
      calls.push(`open ${name} ${epoch}`);
      status = { state: "live" };
      running = true;
    },
    startPresenting: (options) => void calls.push(`present ${options?.latencyMs ?? "-"}`),
    stopPresenting: () => void calls.push("stop"),
    getStatus: () => status,
    getPresentStats: () => ({ running, errors: running ? 0 : 1 }),
    close: () => void calls.push("close"),
    ...over,
  };
  return {
    bridge,
    calls,
    end(reason = "writer-closed") {
      status = { state: "ended", reason };
    },
    crashLoop() {
      running = false;
    },
  };
}

function manualWatch() {
  let cb: (() => void) | null = null;
  const timers: WatchTimers = {
    setInterval: (fn) => {
      cb = fn;
      return 1;
    },
    clearInterval: () => {
      cb = null;
    },
  };
  return {
    timers,
    async tick() {
      cb?.();
      for (let i = 0; i < 5; i++) await Promise.resolve();
    },
    get active() {
      return cb !== null;
    },
  };
}

const canvas = {} as HTMLCanvasElement;

describe("parseRingAnnouncement", () => {
  it("reads {name, epoch} as an object or a JSON string", () => {
    expect(parseRingAnnouncement(RING)).toEqual(RING);
    expect(parseRingAnnouncement(JSON.stringify(RING))).toEqual(RING);
    expect(parseRingAnnouncement({ ...RING, epoch: 255 })).toEqual({ ...RING, epoch: "255" });
    expect(parseRingAnnouncement({ ...RING, layout_version: 1, mapping_size: 9 })).toEqual(RING);
  });

  it("rejects anything else", () => {
    for (const bad of [null, undefined, "", "nope", 3, {}, { name: "" }, { name: "x" }]) {
      expect(parseRingAnnouncement(bad)).toBeNull();
    }
    expect(parseRingAnnouncement({ name: "a b", epoch: "1" })).toBeNull();
    expect(parseRingAnnouncement({ name: "x".repeat(65), epoch: "1" })).toBeNull();
  });
});

describe("RingPreview", () => {
  it("attaches the canvas, opens the announced ring and presents with the clock margin", async () => {
    const f = fakeBridge();
    const w = manualWatch();
    const states: RingPreviewState[] = [];
    const ring = new RingPreview({
      bridge: f.bridge,
      announce: async () => RING,
      timers: w.timers,
      onState: (s) => states.push(s),
    });
    const state = await ring.attach(canvas, RING);
    expect(state).toEqual({ kind: "presenting", adapter: "Fake GPU" });
    expect(f.calls).toEqual(["attach", `open ${RING.name} ${RING.epoch}`, "present 20"]);
    expect(w.active).toBe(true);
    expect(ring.redraw()).toBe(true);
    ring.detach();
    expect(f.calls.slice(-4)).toEqual(["redraw", "stop", "close", "detach"]);
    expect(w.active).toBe(false);
    expect(states.map((s) => s.kind)).toEqual(["presenting", "idle"]);
  });

  it("falls back without the addon, without WebGPU and without a ring", async () => {
    const none = new RingPreview({
      bridge: fakeBridge({ isAvailable: () => false }).bridge,
      announce: async () => null,
    });
    expect(await none.attach(canvas, RING)).toMatchObject({ kind: "failed", reason: "no-addon" });

    const noGpu = fakeBridge({
      attachCanvas: async () => ({ ok: false as const, reason: "no-adapter", message: "none" }),
    });
    const gpu = new RingPreview({ bridge: noGpu.bridge, announce: async () => null });
    expect(await gpu.attach(canvas, RING)).toMatchObject({
      kind: "failed",
      reason: "webgpu",
      message: "none",
    });

    const noRing = fakeBridge();
    const r = new RingPreview({ bridge: noRing.bridge, announce: async () => null });
    expect(await r.attach(canvas, null)).toMatchObject({ kind: "failed", reason: "no-ring" });
    expect(noRing.calls).toEqual(["attach", "detach"]);
  });

  it("a detach while the GPU device is being created wins over the late attach", async () => {
    let finish: (r: { ok: true; adapter: string; upload: string }) => void = () => {};
    const f = fakeBridge({
      attachCanvas: () =>
        new Promise((r) => {
          finish = r;
        }),
    });
    const r = new RingPreview({ bridge: f.bridge, announce: async () => null });
    const pending = r.attach(canvas, RING);
    r.detach();
    finish({ ok: true, adapter: "late", upload: "writeTexture" });
    expect(await pending).toEqual({ kind: "idle" });
    expect(f.calls).toEqual(["detach"]);
  });

  it("an attach that throws (a bad canvas) is a WebGPU failure, not a crash", async () => {
    const f = fakeBridge({
      attachCanvas: async () => {
        throw new Error("boom");
      },
    });
    const r = new RingPreview({ bridge: f.bridge, announce: async () => null });
    expect(await r.attach(canvas, RING)).toMatchObject({ kind: "failed", reason: "webgpu" });
  });

  it("an announced ring that moved before it was opened is replaced by the current one", async () => {
    const moved = { name: "cwp-00001234-00000000000000cc", epoch: "0x02" };
    const f = fakeBridge();
    const open = f.bridge.open;
    f.bridge.open = (name, epoch) => {
      if (name === RING.name) throw new Error("ERR_RING_NOT_FOUND: gone");
      return open(name, epoch);
    };
    f.bridge.getStatus = () => {
      if (!f.calls.some((c) => c.startsWith(`open ${moved.name}`))) {
        throw new Error("ERR_RING_CLOSED: no preview ring is open");
      }
      return { state: "live" };
    };
    const w = manualWatch();
    const r = new RingPreview({ bridge: f.bridge, announce: async () => moved, timers: w.timers });
    expect(await r.attach(canvas, RING)).toEqual({ kind: "recovering" });
    await w.tick();
    expect(r.state).toEqual({ kind: "presenting", adapter: "Fake GPU" });
    expect(f.calls.slice(-2)).toEqual([`open ${moved.name} ${moved.epoch}`, "present 20"]);
  });

  it("a ring that keeps failing to open is a ring failure after three attempts", async () => {
    const f = fakeBridge({
      open: () => {
        throw new Error("ERR_RING_EPOCH: stale");
      },
      getStatus: () => {
        throw new Error("ERR_RING_CLOSED: no preview ring is open");
      },
    });
    const w = manualWatch();
    const r = new RingPreview({ bridge: f.bridge, announce: async () => RING, timers: w.timers });
    expect(await r.attach(canvas, RING)).toEqual({ kind: "recovering" });
    await w.tick();
    expect(r.state.kind).toBe("recovering");
    await w.tick();
    expect(r.state).toMatchObject({
      kind: "failed",
      reason: "ring",
      message: "ERR_RING_EPOCH: stale",
    });
    expect(w.active).toBe(false);
  });

  it("an ended ring is replaced by the engine's current one", async () => {
    const f = fakeBridge();
    const w = manualWatch();
    const next = { name: "cwp-00001234-00000000000000bb", epoch: "0x01" };
    const announce = vi.fn(async () => next);
    const r = new RingPreview({ bridge: f.bridge, announce, timers: w.timers });
    await r.attach(canvas, RING);
    await w.tick();
    expect(announce).not.toHaveBeenCalled();
    f.end("successor-gone");
    await w.tick();
    expect(announce).toHaveBeenCalledTimes(1);
    expect(f.calls.slice(-2)).toEqual([`open ${next.name} ${next.epoch}`, "present 20"]);
    expect(r.state).toEqual({ kind: "presenting", adapter: "Fake GPU" });
  });

  it("a present loop that stopped on an error restarts on the current ring", async () => {
    const f = fakeBridge();
    const w = manualWatch();
    const r = new RingPreview({ bridge: f.bridge, announce: async () => RING, timers: w.timers });
    await r.attach(canvas, RING);
    f.crashLoop();
    await w.tick();
    expect(f.calls.filter((c) => c.startsWith("open"))).toHaveLength(2);
  });

  it("gives up after repeated failures in a row", async () => {
    const f = fakeBridge();
    const w = manualWatch();
    const r = new RingPreview({ bridge: f.bridge, announce: async () => null, timers: w.timers });
    await r.attach(canvas, RING);
    f.end();
    await w.tick();
    expect(r.state).toMatchObject({ kind: "failed", reason: "no-ring" });
    expect(w.active).toBe(false);
  });

  it("setRing() switches to a newly announced ring (engine restart) and ignores the same one", async () => {
    const f = fakeBridge();
    const w = manualWatch();
    const r = new RingPreview({ bridge: f.bridge, announce: async () => null, timers: w.timers });
    await r.attach(canvas, RING);
    r.setRing(RING);
    expect(f.calls.filter((c) => c.startsWith("open"))).toHaveLength(1);
    const next = { name: "cwp-00009999-0000000000000001", epoch: "0x2" };
    r.setRing(next);
    expect(f.calls.slice(-2)).toEqual([`open ${next.name} ${next.epoch}`, "present 20"]);
  });
});
