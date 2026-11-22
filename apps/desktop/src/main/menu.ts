// SPDX-License-Identifier: GPL-3.0-or-later
// Native menu skeleton. Accelerators route to the renderer as validated `menu:command` events.

import type { MenuItemConstructorOptions } from "electron";
import type { MenuCommand } from "../shared/api.ts";

export interface MenuDeps {
  platform: NodeJS.Platform;
  appName: string;
  isDev: boolean;
  /** Send a command to the focused editor window. */
  send(command: MenuCommand): void;
}

export function buildMenuTemplate(deps: MenuDeps): MenuItemConstructorOptions[] {
  const { send, platform } = deps;
  const mac = platform === "darwin";
  const item = (
    label: string,
    command: MenuCommand,
    accelerator: string,
  ): MenuItemConstructorOptions => ({
    label,
    accelerator,
    click: () => send(command),
  });

  const template: MenuItemConstructorOptions[] = [];
  if (mac) template.push({ label: deps.appName, role: "appMenu" });
  template.push(
    {
      label: "&File",
      submenu: [
        item("&Open Project…", "open", "CmdOrCtrl+O"),
        item("&Save", "save", "CmdOrCtrl+S"),
        item("&Export…", "export", "CmdOrCtrl+M"),
        { type: "separator" },
        mac ? { role: "close" } : { role: "quit" },
      ],
    },
    {
      label: "&Edit",
      submenu: [
        item("&Undo", "undo", "CmdOrCtrl+Z"),
        item("&Redo", "redo", mac ? "Cmd+Shift+Z" : "Ctrl+Y"),
        { type: "separator" },
        { role: "cut" },
        { role: "copy" },
        { role: "paste" },
        { role: "selectAll" },
      ],
    },
    {
      label: "&View",
      submenu: [
        { role: "togglefullscreen" },
        ...(deps.isDev
          ? ([{ type: "separator" }, { role: "reload" }, { role: "toggleDevTools" }] as const)
          : []),
      ],
    },
    { label: "&Help", submenu: [{ label: `About ${deps.appName}`, role: "about" }] },
  );
  return template;
}
