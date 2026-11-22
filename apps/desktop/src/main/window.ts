// SPDX-License-Identifier: GPL-3.0-or-later
import path from "node:path";
import { BrowserWindow, screen } from "electron";
import { APP_ORIGIN } from "./app-protocol.ts";
import { mainWindowPreferences } from "./window-prefs.ts";
import {
  DEFAULT_WINDOW_STATE,
  fitToDisplays,
  loadWindowState,
  MIN_WINDOW,
  saveWindowState,
} from "./window-state.ts";

export interface MainWindowOptions {
  /** Directory holding `preload/index.cjs` (the bundle dir). */
  bundleDir: string;
  /** File used to persist bounds. */
  stateFile: string;
  /** Dev only: Vite dev server URL (HMR); production always loads `app://cutweave/`. */
  devServerUrl: string | undefined;
}

/** True for URLs the main window may navigate to. */
export function isAllowedNavigation(url: string, devServerUrl: string | undefined): boolean {
  if (url.startsWith(`${APP_ORIGIN}/`)) return true;
  if (devServerUrl === undefined) return false;
  try {
    return new URL(url).origin === new URL(devServerUrl).origin;
  } catch {
    return false;
  }
}

export function createMainWindow(options: MainWindowOptions): BrowserWindow {
  const saved = loadWindowState(options.stateFile);
  const fitted = fitToDisplays(
    saved ?? DEFAULT_WINDOW_STATE,
    screen.getAllDisplays().map((d) => d.workArea),
  );

  const win = new BrowserWindow({
    ...fitted.bounds,
    minWidth: MIN_WINDOW.width,
    minHeight: MIN_WINDOW.height,
    show: false,
    title: "Cutweave",
    webPreferences: mainWindowPreferences(path.join(options.bundleDir, "preload", "index.cjs")),
  });
  if (fitted.maximized) win.maximize();
  win.once("ready-to-show", () => win.show());

  // Persist bounds on change (debounced) and on close.
  let timer: NodeJS.Timeout | undefined;
  const persist = () => {
    if (win.isDestroyed()) return;
    saveWindowState(options.stateFile, {
      bounds: win.isMaximized() || win.isMinimized() ? win.getNormalBounds() : win.getBounds(),
      maximized: win.isMaximized(),
    });
  };
  const schedule = () => {
    clearTimeout(timer);
    timer = setTimeout(persist, 400);
  };
  win.on("resize", schedule);
  win.on("move", schedule);
  win.on("close", () => {
    clearTimeout(timer);
    persist();
  });

  // Baseline containment; WP-1L.02 owns the full policy.
  win.webContents.setWindowOpenHandler(() => ({ action: "deny" }));
  win.webContents.on("will-navigate", (event, url) => {
    if (!isAllowedNavigation(url, options.devServerUrl)) event.preventDefault();
  });

  void win.loadURL(options.devServerUrl ?? `${APP_ORIGIN}/`);
  return win;
}
