// SPDX-License-Identifier: GPL-3.0-or-later
import type { MenuItemConstructorOptions } from "electron";
import { describe, expect, it, vi } from "vitest";
import type { MenuCommand } from "../shared/api.ts";
import { buildMenuTemplate } from "./menu.ts";

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
    for (const accel of ["CmdOrCtrl+Z", "Ctrl+Y", "CmdOrCtrl+S", "CmdOrCtrl+O", "CmdOrCtrl+M"]) {
      byAccel.get(accel)?.click?.({} as never, undefined, {} as never);
    }
    expect(send.mock.calls.map((c) => c[0])).toEqual(["undo", "redo", "save", "open", "export"]);
  });

  it("has a File menu first and dev tools only in dev", () => {
    const build = (isDev: boolean) =>
      flatten(buildMenuTemplate({ platform: "linux", appName: "Cutweave", isDev, send: () => {} }));
    expect(build(false)[0]?.label).toBe("&File");
    expect(build(false).some((i) => i.role === "toggleDevTools")).toBe(false);
    expect(build(true).some((i) => i.role === "toggleDevTools")).toBe(true);
  });
});
