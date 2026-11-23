// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from "vitest";
import { createHandlers } from "./handlers.ts";

describe("createHandlers", () => {
  const base = {
    appInfo: { name: "Cutweave", version: "1", platform: "linux" as const },
    engineStatus: () => ({ state: "unavailable" as const }),
    chooseFolder: async () => "/p",
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

  it("opens only https links, re-checked at the point of use", async () => {
    const h = createHandlers({ ...base, engineCredentials: () => null });
    await h["shell:openExternal"]("https://cutweave.org/");
    expect(base.openExternal).toHaveBeenCalledWith("https://cutweave.org/");
    await expect(h["shell:openExternal"]("file:///etc/passwd")).rejects.toThrow(/non-https/);
    expect(base.openExternal).toHaveBeenCalledTimes(1);
  });
});
