// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from "vitest";
import {
  createHandlers,
  DevOnlyError,
  MAX_MEDIA_PATHS,
  MAX_SETTINGS_FILE_BYTES,
  MEDIA_FILE_FILTERS,
  validExportPath,
  validMediaPaths,
  validSettingsExportPath,
} from "./handlers.ts";

describe("createHandlers", () => {
  const base = {
    appInfo: { name: "Cutweave", version: "1", platform: "linux" as const },
    engineStatus: () => ({ state: "unavailable" as const }),
    chooseFolder: async () => "/p",
    recentProjects: { list: () => ["/a", "/b"], add: vi.fn(), remove: vi.fn(), clear: vi.fn() },
    setWindowDirty: vi.fn(),
    confirmWindowClose: vi.fn(),
    chooseMediaFiles: vi.fn(async (): Promise<readonly string[] | null> => ["/m/a.mp4"]),
    chooseExportFile: vi.fn(
      async (_defaultPath: string): Promise<string | null> => "/out/film.mp4",
    ),
    showItemInFolder: vi.fn(),
    openExternal: vi.fn(async () => {}),
    about: () => ({
      name: "Cutweave",
      version: "1",
      electron: "45.4.1",
      chrome: "1",
      node: "24",
      v8: "1",
      platform: "linux x64",
    }),
    quit: vi.fn(),
    toggleFullScreen: vi.fn(),
    setTheme: vi.fn(),
    editCommand: vi.fn(),
    prefs: {
      snapshot: vi.fn(() => ({ values: {}, defaults: {} }) as never),
      set: vi.fn(),
      reset: vi.fn(),
      exportText: vi.fn(() => ({ text: '{"version":1,"app":{}}\n', count: 0 })),
      importText: vi.fn(() => ({ status: "imported" as const, applied: [], skipped: [] })),
    },
    chooseSettingsExportFile: vi.fn(async (_name: string): Promise<string | null> => "/out/s.json"),
    chooseSettingsImportFile: vi.fn(async (): Promise<string | null> => "/in/s.json"),
    writeTextFile: vi.fn(),
    readTextFile: vi.fn((_file: string, _max: number): string | null => '{"version":1,"app":{}}'),
    openAppFolder: vi.fn(async () => {}),
    agents: {
      chooseFolder: vi.fn(async (): Promise<string | null> => "/footage"),
      resolveFolder: vi.fn((chosen: string): { ok: true; folder: string } | { ok: false } => ({
        ok: true,
        folder: `/real${chosen}`,
      })),
      folders: vi.fn((): readonly string[] => ["/real/old"]),
      add: (folders: readonly string[], folder: string) =>
        folders.includes(folder)
          ? { status: "exists" as const, folder }
          : { status: "added" as const, folder, folders: [...folders, folder] },
      store: vi.fn(async (_folders: readonly string[]) => {}),
      connectInfo: vi.fn(() => ({
        cliPath: "/opt/cutweave/cutweave",
        setups: [{ client: "claude-code" as const, text: "claude mcp add cutweave -- x" }],
      })),
      writeClipboard: vi.fn(),
    },
  };

  describe("agents (WP-1J.05)", () => {
    const fresh = () => {
      const agents = {
        ...base.agents,
        chooseFolder: vi.fn(async (): Promise<string | null> => "/footage"),
        resolveFolder: vi.fn((chosen: string): { ok: true; folder: string } | { ok: false } => ({
          ok: true,
          folder: `/real${chosen}`,
        })),
        store: vi.fn(async (_folders: readonly string[]) => {}),
        writeClipboard: vi.fn(),
      };
      return { agents, h: createHandlers({ ...base, agents, engineCredentials: () => null }) };
    };

    it("adds the folder main's picker returned, resolved, and waits for the engine", async () => {
      const { agents, h } = fresh();
      expect(await h["agents:addFolder"]()).toEqual({ status: "added", folder: "/real/footage" });
      expect(agents.store).toHaveBeenCalledWith(["/real/old", "/real/footage"]);
    });

    it("stores nothing when the picker is cancelled, the folder is bad or listed", async () => {
      const { agents, h } = fresh();
      agents.chooseFolder.mockResolvedValueOnce(null);
      expect(await h["agents:addFolder"]()).toEqual({ status: "cancelled" });
      agents.resolveFolder.mockReturnValueOnce({ ok: false });
      expect(await h["agents:addFolder"]()).toEqual({ status: "invalid" });
      agents.chooseFolder.mockResolvedValueOnce("/old");
      expect(await h["agents:addFolder"]()).toEqual({ status: "exists", folder: "/real/old" });
      expect(agents.store).not.toHaveBeenCalled();
    });

    it("removes only a stored folder", async () => {
      const { agents, h } = fresh();
      await h["agents:removeFolder"]("/somewhere/else");
      expect(agents.store).not.toHaveBeenCalled();
      await h["agents:removeFolder"]("/real/old");
      expect(agents.store).toHaveBeenCalledWith([]);
    });

    it("copies the setup main built for the client", async () => {
      const { agents, h } = fresh();
      expect(await h["agents:copySetup"]("claude-code")).toBe("claude mcp add cutweave -- x");
      expect(agents.writeClipboard).toHaveBeenCalledWith("claude mcp add cutweave -- x");
      expect(() => h["agents:copySetup"]("cursor")).toThrow();
    });
  });

  it("returns no connection until the engine is ready", async () => {
    const h = createHandlers({ ...base, engineCredentials: () => null });
    expect(await h["engine:getConnection"]()).toBeNull();
  });

  it("builds loopback URLs from the engine port; the token stays out of them", async () => {
    const h = createHandlers({
      ...base,
      engineCredentials: () => ({ port: 4321, token: "t0k", generation: 2 }),
    });
    const c = await h["engine:getConnection"]();
    expect(c).toMatchObject({
      port: 4321,
      token: "t0k",
      httpUrl: "http://127.0.0.1:4321",
      wsUrl: "ws://127.0.0.1:4321/rpc",
      generation: 2,
    });
    expect(`${c?.httpUrl}${c?.wsUrl}`).not.toContain("t0k");
  });

  it("delegates dialogs and shell", async () => {
    const h = createHandlers({ ...base, engineCredentials: () => null });
    expect(await h["dialog:openProjectFolder"]()).toBe("/p");
    await h["shell:showItemInFolder"]("/out.mp4");
    expect(base.showItemInFolder).toHaveBeenCalledWith("/out.mp4");
  });

  it("delegates the recent list and the window-close handshake (WP-1M.07)", async () => {
    const h = createHandlers({ ...base, engineCredentials: () => null });
    expect(await h["recent:list"]()).toEqual(["/a", "/b"]);
    await h["recent:add"]("/c");
    await h["recent:remove"]("/a");
    await h["recent:clear"]();
    await h["window:setDirty"](true);
    await h["window:confirmClose"]();
    expect(base.recentProjects.add).toHaveBeenCalledWith("/c");
    expect(base.recentProjects.remove).toHaveBeenCalledWith("/a");
    expect(base.recentProjects.clear).toHaveBeenCalledTimes(1);
    expect(base.setWindowDirty).toHaveBeenCalledWith(true);
    expect(base.confirmWindowClose).toHaveBeenCalledTimes(1);
  });

  it("delegates the title bar and menu bar actions (WP-1M.09)", async () => {
    const dev = { reload: vi.fn(), toggleDevTools: vi.fn(), inspectElement: vi.fn() };
    const h = createHandlers({ ...base, engineCredentials: () => null, dev });
    expect(await h["app:about"]()).toMatchObject({ name: "Cutweave", electron: "45.4.1" });
    await h["app:quit"]();
    await h["window:toggleFullScreen"]();
    await h["window:setTheme"]("light");
    await h["edit:run"]("selectAll");
    await h["window:reload"]();
    await h["window:toggleDevTools"]();
    await h["window:inspectElement"](12, 34);
    expect(base.quit).toHaveBeenCalledTimes(1);
    expect(base.toggleFullScreen).toHaveBeenCalledTimes(1);
    expect(base.setTheme).toHaveBeenCalledWith("light");
    expect(base.editCommand).toHaveBeenCalledWith("selectAll");
    expect(dev.reload).toHaveBeenCalledTimes(1);
    expect(dev.toggleDevTools).toHaveBeenCalledTimes(1);
    expect(dev.inspectElement).toHaveBeenCalledWith(12, 34);
  });

  it("refuses Reload and developer tools in packaged builds", () => {
    const h = createHandlers({ ...base, engineCredentials: () => null });
    expect(() => h["window:reload"]()).toThrow(DevOnlyError);
    expect(() => h["window:toggleDevTools"]()).toThrow(
      /window:toggleDevTools is only available in development builds/,
    );
    expect(() => h["window:inspectElement"](1, 2)).toThrow(DevOnlyError);
  });

  it("answers the media picker with validated absolute paths, or null when cancelled", async () => {
    const h = createHandlers({ ...base, engineCredentials: () => null });
    expect(await h["dialog:pickMediaFiles"]()).toEqual(["/m/a.mp4"]);
    base.chooseMediaFiles.mockResolvedValueOnce(null);
    expect(await h["dialog:pickMediaFiles"]()).toBeNull();
    base.chooseMediaFiles.mockResolvedValueOnce([
      "/m/a.mp4",
      "/m/a.mp4",
      "relative/b.mp4",
      "",
      "/m/\u0000.mp4",
      `/${"x".repeat(4096)}`,
      "/m/c.wav",
    ]);
    expect(await h["dialog:pickMediaFiles"]()).toEqual(["/m/a.mp4", "/m/c.wav"]);
  });

  it("answers the export picker with one absolute .mp4 path, or null when cancelled", async () => {
    const h = createHandlers({ ...base, engineCredentials: () => null });
    expect(await h["dialog:chooseExportFile"]("/p/film.mp4")).toBe("/out/film.mp4");
    expect(base.chooseExportFile).toHaveBeenCalledWith("/p/film.mp4");
    base.chooseExportFile.mockResolvedValueOnce("/out/film");
    expect(await h["dialog:chooseExportFile"]("film.mp4")).toBe("/out/film.mp4");
    base.chooseExportFile.mockResolvedValueOnce(null);
    expect(await h["dialog:chooseExportFile"]("film.mp4")).toBeNull();
    expect(validExportPath("relative/film.mp4")).toBeNull();
    expect(validExportPath("/out/ .mp4")).toBeNull();
    expect(validExportPath(`/${"x".repeat(4096)}`)).toBeNull();
    expect(validExportPath(7)).toBeNull();
    expect(validExportPath("/out/Film.MP4")).toBe("/out/Film.MP4");
  });

  it("writes the exported settings to the chosen .json file, or nothing when cancelled", async () => {
    const h = createHandlers({ ...base, engineCredentials: () => null });
    expect(await h["prefs:export"]()).toEqual({ status: "saved", path: "/out/s.json", count: 0 });
    expect(base.chooseSettingsExportFile).toHaveBeenCalledWith("cutweave-settings.json");
    expect(base.writeTextFile).toHaveBeenCalledWith("/out/s.json", '{"version":1,"app":{}}\n');
    base.chooseSettingsExportFile.mockResolvedValueOnce("/out/mine");
    expect(await h["prefs:export"]()).toMatchObject({ path: "/out/mine.json" });
    base.writeTextFile.mockClear();
    base.chooseSettingsExportFile.mockResolvedValueOnce(null);
    expect(await h["prefs:export"]()).toEqual({ status: "cancelled" });
    base.chooseSettingsExportFile.mockResolvedValueOnce("relative.json");
    expect(await h["prefs:export"]()).toEqual({ status: "cancelled" });
    expect(base.writeTextFile).not.toHaveBeenCalled();
    expect(validSettingsExportPath("/a/b\u0000.json")).toBeNull();
  });

  it("imports the chosen file through the store, and reports an unreadable one", async () => {
    const h = createHandlers({ ...base, engineCredentials: () => null });
    expect(await h["prefs:import"]()).toEqual({ status: "imported", applied: [], skipped: [] });
    expect(base.readTextFile).toHaveBeenCalledWith("/in/s.json", MAX_SETTINGS_FILE_BYTES);
    base.readTextFile.mockReturnValueOnce(null);
    expect(await h["prefs:import"]()).toEqual({ status: "invalid", reason: "unreadable" });
    base.chooseSettingsImportFile.mockResolvedValueOnce(null);
    expect(await h["prefs:import"]()).toEqual({ status: "cancelled" });
  });

  it("delegates preference reads, writes and resets, and the app folders", async () => {
    const h = createHandlers({ ...base, engineCredentials: () => null });
    await h["prefs:get"]();
    await h["prefs:set"]("appearance.theme", "light");
    await h["prefs:reset"](["saving.autosave"]);
    await h["shell:openAppFolder"]("settings");
    expect(base.prefs.set).toHaveBeenCalledWith("appearance.theme", "light");
    expect(base.prefs.reset).toHaveBeenCalledWith(["saving.autosave"]);
    expect(base.openAppFolder).toHaveBeenCalledWith("settings");
  });

  it("caps the picked files at the engine's limit of one call", () => {
    const many = Array.from({ length: MAX_MEDIA_PATHS + 50 }, (_, i) => `/m/${i}.mp4`);
    expect(validMediaPaths(many)).toHaveLength(MAX_MEDIA_PATHS);
    expect(validMediaPaths([1, null, {}, "/ok.mp4"])).toEqual(["/ok.mp4"]);
  });

  it("offers the media types and, last, every file", () => {
    expect(MEDIA_FILE_FILTERS[0]?.extensions).toEqual(
      expect.arrayContaining(["mp4", "wav", "png"]),
    );
    expect(MEDIA_FILE_FILTERS.at(-1)?.extensions).toEqual(["*"]);
  });

  it("opens only https links, re-checked at the point of use", async () => {
    const h = createHandlers({ ...base, engineCredentials: () => null });
    await h["shell:openExternal"]("https://cutweave.org/");
    expect(base.openExternal).toHaveBeenCalledWith("https://cutweave.org/");
    await expect(h["shell:openExternal"]("file:///etc/passwd")).rejects.toThrow(/non-https/);
    expect(base.openExternal).toHaveBeenCalledTimes(1);
  });
});
