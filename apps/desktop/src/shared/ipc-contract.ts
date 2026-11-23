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
  "recent:list": { args: []; result: string[] };
  "recent:add": { args: [path: string]; result: undefined };
  "recent:remove": { args: [path: string]; result: undefined };
  "window:setDirty": { args: [dirty: boolean]; result: undefined };
  "window:confirmClose": { args: []; result: undefined };
  "shell:showItemInFolder": { args: [path: string]; result: undefined };
  "shell:openExternal": { args: [url: string]; result: undefined };
}

/** Main -> renderer events (`webContents.send`). */
export interface EventChannels {
  "engine:status": EngineStatus;
  "menu:command": MenuCommand;
  /** The window was asked to close while the project has unsaved changes (payload: `true`). */
  "window:closeRequested": true;
}

export type InvokeChannel = keyof InvokeChannels;
export type EventChannel = keyof EventChannels;

export function isMenuCommand(value: unknown): value is MenuCommand {
  return typeof value === "string" && (MENU_COMMANDS as readonly string[]).includes(value);
}
