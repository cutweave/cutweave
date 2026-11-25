// SPDX-License-Identifier: GPL-3.0-or-later
// IPC contract between main and the editor window's preload: channel names and their types.
// Types plus one tiny guard only, so the preload bundle stays free of the validation library.
// The runtime validators (zod) live in main (src/main/ipc.ts) and are type-checked against this.

import type { PreferenceChanges } from "../../../../packages/preferences/src/index.ts";
import {
  type AboutInfo,
  type AgentClient,
  type AgentConnectInfo,
  type AgentFolderResult,
  type AppFolder,
  type AppInfo,
  type EditAction,
  type EngineConnection,
  type EngineStatus,
  MENU_COMMANDS,
  type MenuCommand,
  type PrefsExportResult,
  type PrefsImportResult,
  type PrefsSnapshot,
  type ThemeId,
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
  "recent:clear": { args: []; result: undefined };
  "window:setDirty": { args: [dirty: boolean]; result: undefined };
  "window:confirmClose": { args: []; result: undefined };
  /** Native multi-file picker for media import (WP-1M.03): absolute paths, or null when cancelled. */
  "dialog:pickMediaFiles": { args: []; result: string[] | null };
  /**
   * Native "save as" for the exported video (WP-1M.08): starts at the default path (or file
   * name); resolves to an absolute `.mp4` path, or null when cancelled.
   */
  "dialog:chooseExportFile": { args: [defaultPath: string]; result: string | null };
  "shell:showItemInFolder": { args: [path: string]; result: undefined };
  "shell:openExternal": { args: [url: string]; result: undefined };
  // The title bar and in-app menu bar (WP-1M.09): actions only main can take.
  "app:about": { args: []; result: AboutInfo };
  "app:quit": { args: []; result: undefined };
  "window:toggleFullScreen": { args: []; result: undefined };
  "window:setTheme": { args: [theme: ThemeId]; result: undefined };
  /** Development builds only; refused when packaged. */
  "window:reload": { args: []; result: undefined };
  /** Development builds only; refused when packaged. */
  "window:toggleDevTools": { args: []; result: undefined };
  /** Development builds only; refused when packaged. Page coordinates in CSS pixels. */
  "window:inspectElement": { args: [x: number, y: number]; result: undefined };
  "edit:run": { args: [action: EditAction]; result: undefined };
  // The preferences store (WP-1M.11).
  "prefs:get": { args: []; result: PrefsSnapshot };
  "prefs:set": { args: [key: string, value: unknown]; result: undefined };
  "prefs:reset": { args: [keys: string[]]; result: undefined };
  "prefs:export": { args: [only?: "shortcuts"]; result: PrefsExportResult };
  "prefs:import": { args: [only?: "shortcuts"]; result: PrefsImportResult };
  "shell:openAppFolder": { args: [which: AppFolder]; result: undefined };
  // Settings > Agents and MCP (WP-1J.05).
  /** Main's folder picker; the chosen folder, resolved, joins `agents.allowed_folders`. */
  "agents:addFolder": { args: []; result: AgentFolderResult };
  /** Removes a stored folder (a path not in the list changes nothing). */
  "agents:removeFolder": { args: [folder: string]; result: undefined };
  "agents:connectInfo": { args: []; result: AgentConnectInfo };
  /** Main writes the client's setup to the clipboard and returns it. */
  "agents:copySetup": { args: [client: AgentClient]; result: string };
}

/** Main -> renderer events (`webContents.send`). */
export interface EventChannels {
  "engine:status": EngineStatus;
  "menu:command": MenuCommand;
  /** The window was asked to close while the project has unsaved changes (payload: `true`). */
  "window:closeRequested": true;
  /** The keys that changed in the preferences store, with their new values. */
  "prefs:changed": PreferenceChanges;
}

export type InvokeChannel = keyof InvokeChannels;
export type EventChannel = keyof EventChannels;

export function isMenuCommand(value: unknown): value is MenuCommand {
  return typeof value === "string" && (MENU_COMMANDS as readonly string[]).includes(value);
}
