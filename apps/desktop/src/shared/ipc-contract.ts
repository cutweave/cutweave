// SPDX-License-Identifier: GPL-3.0-or-later
// IPC contract between main and the editor window's preload: channel names and their types.
// Types plus one tiny guard only, so the preload bundle stays free of the validation library.
// The runtime validators (zod) live in main (src/main/ipc.ts) and are type-checked against this.

import {
  type AppInfo,
  type EngineConnection,
  type EngineStatus,
  MENU_COMMANDS,
  type MenuCommand,
} from "./api.ts";

/** Renderer -> main request/response channels (`ipcMain.handle`). */
export interface InvokeChannels {
  "app:getInfo": { args: []; result: AppInfo };
  "engine:getConnection": { args: []; result: EngineConnection | null };
  "engine:getStatus": { args: []; result: EngineStatus };
  "dialog:openProjectFolder": { args: []; result: string | null };
  /** Native multi-file picker for media import (WP-1M.03): absolute paths, or null when cancelled. */
  "dialog:pickMediaFiles": { args: []; result: string[] | null };
  "shell:showItemInFolder": { args: [path: string]; result: undefined };
  "shell:openExternal": { args: [url: string]; result: undefined };
}

/** Main -> renderer events (`webContents.send`). */
export interface EventChannels {
  "engine:status": EngineStatus;
  "menu:command": MenuCommand;
}

export type InvokeChannel = keyof InvokeChannels;
export type EventChannel = keyof EventChannels;

export function isMenuCommand(value: unknown): value is MenuCommand {
  return typeof value === "string" && (MENU_COMMANDS as readonly string[]).includes(value);
}
