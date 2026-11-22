// SPDX-License-Identifier: GPL-3.0-or-later
import path from "node:path";
import { describe, expect, it, vi } from "vitest";
import { createBridgeAddon, defaultAddonPath, loadBridgeAddon } from "./bridge-addon.ts";

function fakeNative() {
  const ring = {
    header: vi.fn(() => ({
      name: "cwp-00000001-0000000000000001",
      epoch: "0x0000000000000001",
      writerPid: 1,
      layoutVersion: 1,
      slotCount: 6,
      slotCapacity: 100,
      mappingSize: 1000,
    })),
    status: vi.fn(),
    latestFrame: vi.fn(() => null),
    frameAtClock: vi.fn(() => null),
    waitForFrame: vi.fn(async () => "timeout" as const),
    audioClock: vi.fn(() => ({
      ptsFlicks: 9,
      state: "playing" as const,
      rateMilli: 1000,
      ageMs: 0,
    })),
    close: vi.fn(),
  };
  return { ring, native: { openRing: vi.fn(() => ring), perfCounters: vi.fn() } };
}

describe("loadBridgeAddon", () => {
  it("returns null and reports (once) when the binary is missing", () => {
    const report = vi.fn();
    const addon = loadBridgeAddon({
      path: "/nowhere/cutweave_bridge.node",
      load: () => {
        throw new Error("Cannot find module '/nowhere/cutweave_bridge.node'");
      },
      report,
    });
    expect(addon).toBeNull();
    expect(report).toHaveBeenCalledTimes(1);
    expect(report.mock.calls[0]?.[0]).toContain("not loaded");
  });

  it("loads from native/ next to dist/ by default", () => {
    expect(defaultAddonPath(path.join("/app", "dist", "preload"))).toBe(
      path.join("/app", "native", "cutweave_bridge.node"),
    );
  });
});

describe("createBridgeAddon", () => {
  it("opens one ring at a time and closes the previous one", () => {
    const { ring, native } = fakeNative();
    const addon = createBridgeAddon(native as never);
    expect(addon.open("r1", "0x1")).toMatchObject({
      name: "cwp-00000001-0000000000000001",
      slotCount: 6,
    });
    addon.open("r2", "0x2");
    expect(ring.close).toHaveBeenCalledTimes(1);
    expect(native.openRing).toHaveBeenLastCalledWith("r2", "0x2");
  });

  it("throws a typed error for calls without an open ring", () => {
    const addon = createBridgeAddon(fakeNative().native as never);
    expect(() => addon.getLatestFrame()).toThrow(/^ERR_RING_CLOSED/);
    expect(() => addon.getStatus()).toThrow(/^ERR_RING_CLOSED/);
  });

  it("a failed open leaves no ring open", () => {
    const { native } = fakeNative();
    const addon = createBridgeAddon(native as never);
    addon.open("a", "1");
    native.openRing.mockImplementationOnce(() => {
      throw new Error("ERR_RING_EPOCH: epoch mismatch");
    });
    expect(() => addon.open("b", "2")).toThrow(/ERR_RING_EPOCH/);
    expect(() => addon.getLatestFrame()).toThrow(/ERR_RING_CLOSED/);
  });

  it("maps the audio clock, null clock included", () => {
    const { ring, native } = fakeNative();
    const addon = createBridgeAddon(native as never);
    addon.open("r", "1");
    expect(addon.getClock()).toEqual({ ptsFlicks: 9, state: "playing", rateMilli: 1000 });
    ring.audioClock.mockReturnValueOnce(null as never);
    expect(addon.getClock()).toEqual({ ptsFlicks: null });
  });
});
