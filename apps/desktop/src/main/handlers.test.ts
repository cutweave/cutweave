// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from "vitest";
import {
  createHandlers,
  MAX_MEDIA_PATHS,
  MEDIA_FILE_FILTERS,
  validMediaPaths,
} from "./handlers.ts";

describe("createHandlers", () => {
  const base = {
    appInfo: { name: "Cutweave", version: "1", platform: "linux" as const },
    engineStatus: () => ({ state: "unavailable" as const }),
    chooseFolder: async () => "/p",
    recentProjects: { list: () => ["/a", "/b"], add: vi.fn(), remove: vi.fn() },
    setWindowDirty: vi.fn(),
    confirmWindowClose: vi.fn(),
    chooseMediaFiles: vi.fn(async (): Promise<readonly string[] | null> => ["/m/a.mp4"]),
    showItemInFolder: vi.fn(),
    openExternal: vi.fn(async () => {}),
  };

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
    await h["window:setDirty"](true);
    await h["window:confirmClose"]();
    expect(base.recentProjects.add).toHaveBeenCalledWith("/c");
    expect(base.recentProjects.remove).toHaveBeenCalledWith("/a");
    expect(base.setWindowDirty).toHaveBeenCalledWith(true);
    expect(base.confirmWindowClose).toHaveBeenCalledTimes(1);
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
