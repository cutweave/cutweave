// SPDX-License-Identifier: GPL-3.0-or-later
// The native application menu: macOS only (WP-1M.09). There it stays at the top of the screen and
// its accelerators reach the renderer as validated `menu:command` events. On Windows and Linux
// the window has no native menu bar: the editor draws File, Edit, View and Help in its title bar
// and handles the shortcuts itself, and main-process actions go through allowlisted IPC.

import type { MenuItemConstructorOptions } from "electron";
import type { MenuCommand } from "../shared/api.ts";

export interface MenuDeps {
  platform: NodeJS.Platform;
  appName: string;
  isDev: boolean;
  /** Send a command to the focused editor window. */
  send(command: MenuCommand): void;
}

/**
 * The application menu for `deps.platform`: the macOS template, or `null` on Windows and Linux,
 * where `Menu.setApplicationMenu(null)` removes the native menu bar from the window.
 */
export function applicationMenuTemplate(deps: MenuDeps): MenuItemConstructorOptions[] | null {
  return deps.platform === "darwin" ? buildMenuTemplate(deps) : null;
}

/** The native menu template (macOS uses it; kept platform-aware for its unit test). */
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
  if (mac) {
    // The app menu as Electron's `appMenu` role builds it, plus Settings… (⌘,) after About.
    template.push({
      label: deps.appName,
      submenu: [
        { role: "about" },
        { type: "separator" },
        item("Settings…", "settings", "Cmd+,"),
        { type: "separator" },
        { role: "services" },
        { type: "separator" },
        { role: "hide" },
        { role: "hideOthers" },
        { role: "unhide" },
        { type: "separator" },
        { role: "quit" },
      ],
    });
  }
  template.push(
    {
      label: "&File",
      submenu: [
        item("&New Project…", "new", "CmdOrCtrl+N"),
        item("&Open Project…", "open", "CmdOrCtrl+O"),
        item("&Save", "save", "CmdOrCtrl+S"),
        item("Save &As…", "saveAs", "CmdOrCtrl+Shift+S"),
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
