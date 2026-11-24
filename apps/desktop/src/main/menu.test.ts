// SPDX-License-Identifier: GPL-3.0-or-later
import type { MenuItemConstructorOptions } from "electron";
import { describe, expect, it, vi } from "vitest";
import type { MenuCommand } from "../shared/api.ts";
import { applicationMenuTemplate, buildMenuTemplate } from "./menu.ts";

function flatten(items: MenuItemConstructorOptions[]): MenuItemConstructorOptions[] {
  return items.flatMap((i) => [i, ...(Array.isArray(i.submenu) ? flatten(i.submenu) : [])]);
}

describe("buildMenuTemplate", () => {
  it("routes accelerators to validated renderer commands", () => {
    const send = vi.fn<(c: MenuCommand) => void>();
    const items = flatten(
      buildMenuTemplate({ platform: "win32", appName: "Cutweave", isDev: false, send }),
    );
    const byAccel = new Map(items.filter((i) => i.accelerator).map((i) => [i.accelerator, i]));
    for (const accel of [
      "CmdOrCtrl+Z",
      "Ctrl+Y",
      "CmdOrCtrl+S",
      "CmdOrCtrl+Shift+S",
      "CmdOrCtrl+N",
      "CmdOrCtrl+O",
      "CmdOrCtrl+M",
    ]) {
      byAccel.get(accel)?.click?.({} as never, undefined, {} as never);
    }
    expect(send.mock.calls.map((c) => c[0])).toEqual([
      "undo",
      "redo",
      "save",
      "saveAs",
      "new",
      "open",
      "export",
    ]);
  });

  it("has a File menu first and dev tools only in dev", () => {
    const build = (isDev: boolean) =>
      flatten(buildMenuTemplate({ platform: "linux", appName: "Cutweave", isDev, send: () => {} }));
    expect(build(false)[0]?.label).toBe("&File");
    expect(build(false).some((i) => i.role === "toggleDevTools")).toBe(false);
    expect(build(true).some((i) => i.role === "toggleDevTools")).toBe(true);
  });
});

describe("applicationMenuTemplate (WP-1M.09)", () => {
  const deps = (platform: NodeJS.Platform) => ({
    platform,
    appName: "Cutweave",
    isDev: true,
    send: () => {},
  });

  it("has no native menu on Windows and Linux (the editor draws its own menu bar)", () => {
    expect(applicationMenuTemplate(deps("win32"))).toBeNull();
    expect(applicationMenuTemplate(deps("linux"))).toBeNull();
  });

  it("keeps the native app menu on macOS, routing accelerators to the renderer", () => {
    const send = vi.fn<(c: MenuCommand) => void>();
    const template = applicationMenuTemplate({ ...deps("darwin"), send });
    expect(template?.map((m) => m.label)).toEqual(["Cutweave", "&File", "&Edit", "&View", "&Help"]);
    const items = flatten(template ?? []);
    items
      .find((i) => i.accelerator === "Cmd+Shift+Z")
      ?.click?.({} as never, undefined, {} as never);
    expect(send).toHaveBeenCalledWith("redo");
    // The app menu: About, then Settings… (⌘,), which the editor opens as its own dialog.
    const appMenu = flatten((template?.[0]?.submenu as MenuItemConstructorOptions[]) ?? []);
    expect(appMenu.map((i) => i.role ?? i.label).slice(0, 3)).toEqual([
      "about",
      undefined,
      "Settings…",
    ]);
    appMenu.find((i) => i.accelerator === "Cmd+,")?.click?.({} as never, undefined, {} as never);
    expect(send).toHaveBeenCalledWith("settings");
    // macOS needs the edit roles: Cmd+C/V/X/A in text fields go through them.
    expect(items.filter((i) => i.role).map((i) => i.role)).toEqual(
      expect.arrayContaining([
        "cut",
        "copy",
        "paste",
        "selectAll",
        "togglefullscreen",
        "about",
        "quit",
      ]),
    );
  });
});
