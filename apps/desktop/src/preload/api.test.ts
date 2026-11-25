// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from "vitest";
import { createCutweaveApi, ERR_BRIDGE_UNAVAILABLE, type IpcRendererLike } from "./api.ts";
import type { BridgeAddon } from "./bridge-addon.ts";
import { PresenterError } from "./webgpu-presenter.ts";

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
      "agents",
      "app",
      "dialog",
      "edit",
      "engine",
      "files",
      "menu",
      "prefs",
      "preview",
      "recent",
      "shell",
      "window",
    ]);
    expect(Object.keys(api.engine).sort()).toEqual(["getConnection", "getStatus", "onStatus"]);
    expect("invoke" in api).toBe(false);
  });

  it("names no folder when adding one for agents, and only a client to copy (WP-1J.05)", async () => {
    const { ipc } = fakeIpc();
    const api = createCutweaveApi(ipc, null);
    expect(Object.keys(api.agents).sort()).toEqual([
      "addFolder",
      "connectInfo",
      "copySetup",
      "removeFolder",
    ]);
    await api.agents.addFolder();
    expect(ipc.invoke).toHaveBeenLastCalledWith("agents:addFolder");
    await api.agents.removeFolder("D:/Footage");
    expect(ipc.invoke).toHaveBeenLastCalledWith("agents:removeFolder", "D:/Footage");
    await api.agents.copySetup("codex");
    expect(ipc.invoke).toHaveBeenLastCalledWith("agents:copySetup", "codex");
    await api.agents.connectInfo();
    expect(ipc.invoke).toHaveBeenLastCalledWith("agents:connectInfo");
  });

  it("asks main for the media picker over its own channel", async () => {
    const { ipc } = fakeIpc();
    await createCutweaveApi(ipc, null).dialog.pickMediaFiles();
    expect(ipc.invoke).toHaveBeenCalledWith("dialog:pickMediaFiles");
  });

  it("asks main for the media folder picker over its own channel", async () => {
    const { ipc } = fakeIpc();
    await createCutweaveApi(ipc, null).dialog.pickMediaFolders();
    expect(ipc.invoke).toHaveBeenCalledWith("dialog:pickMediaFolders");
  });

  it("asks main for the export file picker with the default path", async () => {
    const { ipc } = fakeIpc();
    await createCutweaveApi(ipc, null).dialog.chooseExportFile("C:/p/film.mp4");
    expect(ipc.invoke).toHaveBeenCalledWith("dialog:chooseExportFile", "C:/p/film.mp4");
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

  it("maps the preferences to fixed channels, one named function each (WP-1M.11)", async () => {
    const { ipc, emit } = fakeIpc();
    const api = createCutweaveApi(ipc, null);
    expect(Object.keys(api.prefs).sort()).toEqual([
      "exportToFile",
      "get",
      "importFromFile",
      "onChanged",
      "reset",
      "set",
    ]);
    await api.prefs.get();
    await api.prefs.set("appearance.theme", "light");
    await api.prefs.reset(["saving.autosave"]);
    await api.prefs.exportToFile();
    await api.prefs.importFromFile();
    await api.shell.openAppFolder("logs");
    await api.recent.clear();
    expect(ipc.invoke).toHaveBeenNthCalledWith(1, "prefs:get");
    expect(ipc.invoke).toHaveBeenNthCalledWith(2, "prefs:set", "appearance.theme", "light");
    expect(ipc.invoke).toHaveBeenNthCalledWith(3, "prefs:reset", ["saving.autosave"]);
    expect(ipc.invoke).toHaveBeenNthCalledWith(4, "prefs:export");
    expect(ipc.invoke).toHaveBeenNthCalledWith(5, "prefs:import");
    expect(ipc.invoke).toHaveBeenNthCalledWith(6, "shell:openAppFolder", "logs");
    expect(ipc.invoke).toHaveBeenNthCalledWith(7, "recent:clear");
    // Changes arrive as objects only.
    const seen: unknown[] = [];
    const off = api.prefs.onChanged((changes) => seen.push(changes));
    emit("prefs:changed", { "appearance.theme": "light" });
    emit("prefs:changed", "nope");
    emit("prefs:changed", null);
    emit("prefs:changed", ["x"]);
    expect(seen).toEqual([{ "appearance.theme": "light" }]);
    off();
  });

  it("maps the recent list and the close handshake to fixed channels (WP-1M.07)", async () => {
    const { ipc, emit, listeners } = fakeIpc();
    const api = createCutweaveApi(ipc, null);
    await api.recent.list();
    await api.recent.add("/p");
    await api.window.setDirty(true);
    await api.window.confirmClose();
    expect(ipc.invoke).toHaveBeenNthCalledWith(1, "recent:list");
    expect(ipc.invoke).toHaveBeenNthCalledWith(2, "recent:add", "/p");
    expect(ipc.invoke).toHaveBeenNthCalledWith(3, "window:setDirty", true);
    expect(ipc.invoke).toHaveBeenNthCalledWith(4, "window:confirmClose");
    let asked = 0;
    const off = api.window.onCloseRequested(() => {
      asked += 1;
    });
    emit("window:closeRequested", true);
    emit("window:closeRequested", "junk");
    expect(asked).toBe(1);
    off();
    expect(listeners.get("window:closeRequested")?.size).toBe(0);
  });

  it("maps the title bar and menu bar actions to fixed channels (WP-1M.09)", async () => {
    const { ipc } = fakeIpc();
    const api = createCutweaveApi(ipc, null);
    await api.app.about();
    await api.app.quit();
    await api.window.toggleFullScreen();
    await api.window.setTheme("light");
    await api.window.reload();
    await api.window.toggleDevTools();
    await api.window.inspectElement(12, 34);
    await api.edit.run("paste");
    expect(vi.mocked(ipc.invoke).mock.calls).toEqual([
      ["app:about"],
      ["app:quit"],
      ["window:toggleFullScreen"],
      ["window:setTheme", "light"],
      ["window:reload"],
      ["window:toggleDevTools"],
      ["window:inspectElement", 12, 34],
      ["edit:run", "paste"],
    ]);
    expect(Object.keys(api.edit)).toEqual(["run"]);
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

  it("attachCanvas installs a WebGPU presenter the running loop draws with; detach releases it", async () => {
    const frame = { seq: 1, pts: 0, width: 2, height: 2, format: "rgba8" };
    const addon = { ...fakeAddon(), frameAtClock: vi.fn(() => frame as never) };
    const ticks: (() => void)[] = [];
    const drawn: number[] = [];
    const destroyed: string[] = [];
    const presenter = {
      adapter: "Fake GPU",
      present: (f: { seq: number }) => drawn.push(f.seq),
      redraw: () => true,
      destroy: () => destroyed.push("gpu"),
      stats: () => ({
        kind: "webgpu" as const,
        adapter: "Fake GPU",
        lastPath: "",
        uploads: 0,
        draws: 0,
        errors: 0,
        lastError: null,
        lost: false,
      }),
    };
    const api = createCutweaveApi(
      fakeIpc().ipc,
      addon,
      () => ({ schedule: (t) => ticks.push(t), cancel: () => {}, now: () => 0 }),
      undefined,
      async () => presenter,
    );
    api.preview.startPresenting();
    ticks.shift()?.();
    expect(drawn).toEqual([]);
    expect(api.preview.getPresentStats()?.presenter).toBe("none");
    const canvas = { getContext: () => null } as unknown as HTMLCanvasElement;
    expect(await api.preview.attachCanvas(canvas)).toEqual({
      ok: true,
      adapter: "Fake GPU",
      upload: "writeTexture",
    });
    ticks.shift()?.();
    expect(drawn).toEqual([1]);
    expect(api.preview.getPresentStats()).toMatchObject({
      presenter: "webgpu",
      presenterLost: false,
    });
    expect(api.preview.redraw()).toBe(true);
    api.preview.detachCanvas();
    expect(destroyed).toEqual(["gpu"]);
    expect(api.preview.redraw()).toBe(false);
    ticks.shift()?.();
    expect(drawn).toEqual([1]);
  });

  it("attachCanvas reports why WebGPU cannot present instead of throwing", async () => {
    const none = createCutweaveApi(fakeIpc().ipc, null);
    const canvas = { getContext: () => null } as unknown as HTMLCanvasElement;
    expect(await none.preview.attachCanvas(canvas)).toMatchObject({
      ok: false,
      reason: "no-addon",
    });
    const api = createCutweaveApi(fakeIpc().ipc, fakeAddon(), undefined, undefined, async () => {
      throw new PresenterError("no-adapter", "no WebGPU adapter");
    });
    expect(await api.preview.attachCanvas(canvas)).toEqual({
      ok: false,
      reason: "no-adapter",
      message: "no WebGPU adapter",
    });
    expect(await api.preview.attachCanvas({} as HTMLCanvasElement)).toMatchObject({
      ok: false,
      reason: "bad-canvas",
    });
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
