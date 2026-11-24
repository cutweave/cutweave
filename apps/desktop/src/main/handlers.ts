// SPDX-License-Identifier: GPL-3.0-or-later
// Implementations of the allowlisted IPC channels (src/main/ipc.ts), with Electron injected.
// Arguments arrive already sender-checked and zod-validated.

import nodePath from "node:path";
import type {
  AboutInfo,
  AppInfo,
  EditAction,
  EngineConnection,
  EngineStatus,
  ThemeId,
} from "../shared/api.ts";
import type { InvokeHandlers } from "./ipc.ts";
import { parseExternalUrl } from "./origins.ts";

export interface HandlerDeps {
  appInfo: AppInfo;
  /** Current engine status. */
  engineStatus(): EngineStatus;
  /** Port and UI token of the supervised engine; null when none is ready. */
  engineCredentials(): { port: number; token: string; generation: number } | null;
  chooseFolder(): Promise<string | null>;
  /** Recent projects kept by main (WP-1M.07). */
  recentProjects: {
    list(): string[];
    add(path: string): void;
    remove(path: string): void;
  };
  /** The editor's unsaved-changes flag, for the window-close guard. */
  setWindowDirty(dirty: boolean): void;
  /** The editor settled unsaved changes: close the window without asking again. */
  confirmWindowClose(): void;
  /** Shows the native multi-file picker; resolves to the chosen paths or null when cancelled. */
  chooseMediaFiles(): Promise<readonly string[] | null>;
  showItemInFolder(path: string): void;
  /** `shell.openExternal`; only ever called with a validated `https:` URL. */
  openExternal(url: string): Promise<void>;
  // The title bar and in-app menu bar (WP-1M.09).
  /** Versions for Help > About. */
  about(): AboutInfo;
  /** File > Exit: `app.quit()` (the close guard settles unsaved changes first). */
  quit(): void;
  toggleFullScreen(): void;
  /** Recolors the OS window-controls overlay (Windows, Linux; nothing on macOS). */
  setTheme(theme: ThemeId): void;
  /** `webContents.cut()` and friends on the editor window's focused element. */
  editCommand(action: EditAction): void;
  /** View > Reload and Toggle developer tools: present in unpackaged runs only. */
  dev?: { reload(): void; toggleDevTools(): void } | undefined;
}

/** A development-only action asked for in a packaged build. */
export class DevOnlyError extends Error {
  constructor(channel: string) {
    super(`${channel} is only available in development builds`);
    this.name = "DevOnlyError";
  }
}

/** File-type filters of the media picker; "All files" stays last so nothing is out of reach. */
export const MEDIA_FILE_FILTERS: { name: string; extensions: string[] }[] = [
  {
    name: "Media",
    extensions: [
      ...["mp4", "mov", "m4v", "mkv", "webm", "avi"],
      ...["wav", "mp3", "m4a", "aac", "flac", "ogg", "opus"],
      ...["png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff"],
    ],
  },
  { name: "All files", extensions: ["*"] },
];

/** The most paths one `media.import` takes (engine limit). */
export const MAX_MEDIA_PATHS = 256;
const MAX_PATH_LENGTH = 4096;

/** Keeps absolute, bounded, NUL-free, distinct paths, at most `MAX_MEDIA_PATHS`. */
export function validMediaPaths(paths: readonly unknown[]): string[] {
  const out = new Set<string>();
  for (const p of paths) {
    if (
      typeof p === "string" &&
      p.length > 0 &&
      p.length <= MAX_PATH_LENGTH &&
      !p.includes("\u0000") &&
      nodePath.isAbsolute(p)
    ) {
      out.add(p);
    }
    if (out.size === MAX_MEDIA_PATHS) break;
  }
  return [...out];
}

export function createHandlers(deps: HandlerDeps): InvokeHandlers {
  return {
    "app:getInfo": () => deps.appInfo,
    "engine:getStatus": () => deps.engineStatus(),
    "engine:getConnection": (): EngineConnection | null => {
      const creds = deps.engineCredentials();
      if (creds === null) return null;
      return {
        port: creds.port,
        token: creds.token,
        generation: creds.generation,
        httpUrl: `http://127.0.0.1:${creds.port}`,
        wsUrl: `ws://127.0.0.1:${creds.port}/rpc`,
      };
    },
    "dialog:openProjectFolder": () => deps.chooseFolder(),
    "recent:list": () => deps.recentProjects.list(),
    "recent:add": (path) => {
      deps.recentProjects.add(path);
    },
    "recent:remove": (path) => {
      deps.recentProjects.remove(path);
    },
    "window:setDirty": (dirty) => {
      deps.setWindowDirty(dirty);
    },
    "window:confirmClose": () => {
      deps.confirmWindowClose();
    },
    "dialog:pickMediaFiles": async () => {
      const chosen = await deps.chooseMediaFiles();
      if (chosen === null) return null;
      // Re-check what comes back from the dialog layer: only plain, bounded, absolute paths leave.
      return validMediaPaths(chosen);
    },
    "shell:showItemInFolder": (path) => {
      deps.showItemInFolder(path);
    },
    "shell:openExternal": async (url) => {
      // Re-check at the point of use (defense in depth; the schema already did).
      const safe = parseExternalUrl(url);
      if (safe === null) throw new Error("refusing to open a non-https URL");
      await deps.openExternal(safe);
    },
    "app:about": () => deps.about(),
    "app:quit": () => {
      deps.quit();
    },
    "window:toggleFullScreen": () => {
      deps.toggleFullScreen();
    },
    "window:setTheme": (theme) => {
      deps.setTheme(theme);
    },
    "window:reload": () => {
      if (deps.dev === undefined) throw new DevOnlyError("window:reload");
      deps.dev.reload();
    },
    "window:toggleDevTools": () => {
      if (deps.dev === undefined) throw new DevOnlyError("window:toggleDevTools");
      deps.dev.toggleDevTools();
    },
    "edit:run": (action) => {
      deps.editCommand(action);
    },
  };
}
