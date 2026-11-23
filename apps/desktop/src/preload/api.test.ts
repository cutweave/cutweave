// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from "vitest";
import { createCutweaveApi, ERR_BRIDGE_UNAVAILABLE, type IpcRendererLike } from "./api.ts";
import type { BridgeAddon } from "./bridge-addon.ts";

function fakeIpc() {
  const listeners = new Map<string, Set<(e: unknown, ...a: unknown[]) => void>>();
  const ipc: IpcRendererLike = {
    invoke: vi.fn(async (channel: string) => ({ channel })),
    on: (channel, l) => {
      const set = listeners.get(channel) ?? new Set();
      set.add(l);
      listeners.set(channel, set);
    },
    removeListener: (channel, l) => void listeners.get(channel)?.delete(l),
  };
  const emit = (channel: string, value: unknown) => {
    for (const l of listeners.get(channel) ?? []) l({}, value);
  };
  return { ipc, emit, listeners };
}

describe("createCutweaveApi", () => {
  it("exposes only named, narrow members (no generic invoke or ipcRenderer)", () => {
    const api = createCutweaveApi(fakeIpc().ipc, null);
    expect(Object.keys(api).sort()).toEqual([
      "app",
      "dialog",
      "engine",
      "files",
      "menu",
      "preview",
      "shell",
    ]);
    expect(Object.keys(api.engine).sort()).toEqual(["getConnection", "getStatus", "onStatus"]);
    expect("invoke" in api).toBe(false);
  });

  it("asks main for the media picker over its own channel", async () => {
    const { ipc } = fakeIpc();
    await createCutweaveApi(ipc, null).dialog.pickMediaFiles();
    expect(ipc.invoke).toHaveBeenCalledWith("dialog:pickMediaFiles");
  });

  it("resolves a dropped File to its path through Electron, and to nothing otherwise", () => {
    const { ipc } = fakeIpc();
    const getPath = vi.fn((f: File) => `/drop/${f.name}`);
    const api = createCutweaveApi(ipc, null, undefined, getPath);
    expect(api.files.pathForFile(new File(["x"], "a.mp4"))).toBe("/drop/a.mp4");
    // Anything that is not a real File never reaches Electron.
    expect(api.files.pathForFile({ name: "evil" } as unknown as File)).toBe("");
    expect(api.files.pathForFile("/etc/passwd" as unknown as File)).toBe("");
    expect(getPath).toHaveBeenCalledTimes(1);
    const throwing = createCutweaveApi(ipc, null, undefined, () => {
      throw new Error("not a file on disk");
    });
    expect(throwing.files.pathForFile(new File(["x"], "b.mp4"))).toBe("");
    const nul = createCutweaveApi(ipc, null, undefined, () => "/a\u0000b");
    expect(nul.files.pathForFile(new File(["x"], "c.mp4"))).toBe("");
    const long = createCutweaveApi(ipc, null, undefined, () => `/${"x".repeat(4096)}`);
    expect(long.files.pathForFile(new File(["x"], "d.mp4"))).toBe("");
  });

  it("maps methods to fixed channels", async () => {
    const { ipc } = fakeIpc();
    const api = createCutweaveApi(ipc, null);
    await api.engine.getConnection();
    await api.shell.showItemInFolder("/x");
    await api.shell.openExternal("https://cutweave.org/");
    expect(ipc.invoke).toHaveBeenNthCalledWith(1, "engine:getConnection");
    expect(ipc.invoke).toHaveBeenNthCalledWith(2, "shell:showItemInFolder", "/x");
    expect(ipc.invoke).toHaveBeenNthCalledWith(3, "shell:openExternal", "https://cutweave.org/");
  });

  it("filters menu events to known commands and unsubscribes", () => {
    const { ipc, emit, listeners } = fakeIpc();
    const api = createCutweaveApi(ipc, null);
    const seen: string[] = [];
    const off = api.menu.onCommand((c) => seen.push(c));
    emit("menu:command", "save");
    emit("menu:command", "evil");
    expect(seen).toEqual(["save"]);
    off();
    expect(listeners.get("menu:command")?.size).toBe(0);
  });

  it("delivers engine status events", () => {
    const { ipc, emit } = fakeIpc();
    const api = createCutweaveApi(ipc, null);
    const seen: string[] = [];
    api.engine.onStatus((s) => seen.push(s.state));
    emit("engine:status", { state: "ready", port: 1 });
    emit("engine:status", "junk");
    expect(seen).toEqual(["ready"]);
  });

  it("preview methods throw ERR_BRIDGE_UNAVAILABLE without an addon", () => {
    const api = createCutweaveApi(fakeIpc().ipc, null);
    expect(api.preview.isAvailable()).toBe(false);
    expect(() => api.preview.getLatestFrame()).toThrow(ERR_BRIDGE_UNAVAILABLE);
    expect(() => api.preview.open("r", "0x1")).toThrow(ERR_BRIDGE_UNAVAILABLE);
    expect(() => api.preview.startPresenting()).toThrow(ERR_BRIDGE_UNAVAILABLE);
    expect(api.preview.getPresentStats()).toBeNull();
  });

  it("preview delegates to the addon when present", () => {
    const addon = fakeAddon();
    const api = createCutweaveApi(fakeIpc().ipc, addon);
    api.preview.open("ring", "0xabc");
    expect(addon.open).toHaveBeenCalledWith("ring", "0xabc");
    expect(api.preview.getClock()).toEqual({ ptsFlicks: 5 });
    expect(api.preview.isAvailable()).toBe(true);
    api.preview.getLatestFrame(7);
    expect(addon.getLatestFrame).toHaveBeenCalledWith(7);
    api.preview.close();
    expect(addon.close).toHaveBeenCalled();
  });

  it("the present loop runs in the preload: only stats come back, opening a ring stops it", () => {
    const addon = fakeAddon();
    const ticks: (() => void)[] = [];
    const api = createCutweaveApi(fakeIpc().ipc, addon, () => ({
      schedule: (t) => ticks.push(t),
      cancel: () => {},
      now: () => 0,
    }));
    api.preview.startPresenting({ latencyMs: 12 });
    ticks.shift()?.();
    expect(addon.frameAtClock).toHaveBeenCalledWith(12);
    expect(api.preview.getPresentStats()?.running).toBe(true);
    api.preview.open("ring", "1");
    expect(api.preview.getPresentStats()).toBeNull();
  });
});

function fakeAddon() {
  return {
    open: vi.fn(() => ({ name: "r", epoch: "0x1", writerPid: 1, slotCount: 6, slotCapacity: 1 })),
    getLatestFrame: vi.fn(() => null),
    frameAtClock: vi.fn(() => null),
    waitForFrame: vi.fn(async () => "timeout" as const),
    getClock: vi.fn(() => ({ ptsFlicks: 5 })),
    getStatus: vi.fn(),
    close: vi.fn(),
  } satisfies BridgeAddon;
}
