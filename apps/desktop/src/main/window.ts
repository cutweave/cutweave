// SPDX-License-Identifier: GPL-3.0-or-later
// The main editor window: created through the factory as kind `editor` (the only kind with the
// preload), plus bounds persistence. Navigation and window-open guards are attached to every
// webContents in hardening.ts.

import { type BrowserWindow, screen } from "electron";
import { titleBarOptions } from "./title-bar.ts";
import {
  DEFAULT_WINDOW_STATE,
  fitToDisplays,
  loadWindowState,
  MIN_WINDOW,
  saveWindowState,
} from "./window-state.ts";
import { createWindow, type WindowFactoryContext } from "./windows.ts";

export interface MainWindowOptions {
  /** File used to persist bounds. */
  stateFile: string;
  /** `app://cutweave/` (or, dev only, the Vite dev server URL for HMR). */
  url: string;
  factory: WindowFactoryContext;
}

export function createMainWindow(options: MainWindowOptions): BrowserWindow {
  const saved = loadWindowState(options.stateFile);
  const fitted = fitToDisplays(
    saved ?? DEFAULT_WINDOW_STATE,
    screen.getAllDisplays().map((d) => d.workArea),
  );

  const win = createWindow(
    "editor",
    {
      ...fitted.bounds,
      minWidth: MIN_WINDOW.width,
      minHeight: MIN_WINDOW.height,
      show: false,
      title: "Cutweave",
      // No native title bar: the editor's top bar is the title bar (WP-1M.09, title-bar.ts).
      ...titleBarOptions(process.platform),
      url: options.url,
    },
    options.factory,
  );
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
  return win;
}
