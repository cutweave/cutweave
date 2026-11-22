// SPDX-License-Identifier: GPL-3.0-or-later
// Main-process hooks for the Playwright security suite (e2e/security.spec.ts), reachable only
// from main via Playwright's `electronApp.evaluate` (never from a renderer). Installed only for
// unpackaged runs with CUTWEAVE_TEST_HOOKS=1; `shouldInstallTestHooks` is unit-tested.

import type { BrowserWindow, WebPreferences } from "electron";
import { isWindowKind } from "./window-policy.ts";

export function shouldInstallTestHooks(
  isPackaged: boolean,
  env: Readonly<Record<string, string | undefined>>,
): boolean {
  return !isPackaged && env.CUTWEAVE_TEST_HOOKS === "1";
}

export interface TestHookDeps {
  /** The factory, bound to main's context. */
  createWindow(
    kind: "editor" | "auxiliary",
    options: { url: string; show: boolean; webPreferences?: WebPreferences },
  ): BrowserWindow;
  appUrl: string;
  enginePort(): number | null;
  /** Replace `shell.openExternal` so tests never launch a browser; URLs are pushed to `sink`. */
  captureOpenExternal(sink: string[]): void;
}

export interface CutweaveTestHooks {
  /** Create a window through the factory; throws WindowPolicyError on a policy violation. */
  createWindow(kind: string, webPreferences?: WebPreferences): number;
  /** Factory-made editor window (with the preload) that main then points at `url`. */
  openEditorAt(url: string): number;
  /** Destroy every window these hooks created. */
  closeAll(): void;
  enginePort(): number | null;
  /** From now on `shell:openExternal` records instead of launching a browser. */
  captureOpenExternal(): void;
  /** URLs main would have opened in the OS browser since `captureOpenExternal()`. */
  openedExternal(): string[];
}

declare global {
  var __cutweaveTest: CutweaveTestHooks | undefined;
}

export function installTestHooks(deps: TestHookDeps): void {
  const windows = new Map<number, BrowserWindow>();
  const opened: string[] = [];
  const track = (win: BrowserWindow): number => {
    windows.set(win.id, win);
    win.once("closed", () => windows.delete(win.id));
    return win.id;
  };
  globalThis.__cutweaveTest = {
    createWindow(kind, webPreferences) {
      if (!isWindowKind(kind)) throw new Error(`unknown window kind: ${kind}`);
      return track(
        deps.createWindow(kind, {
          url: deps.appUrl,
          show: false,
          ...(webPreferences ? { webPreferences } : {}),
        }),
      );
    },
    openEditorAt(url) {
      const win = deps.createWindow("editor", { url: deps.appUrl, show: false });
      // Main-initiated loads skip will-navigate; this simulates foreign content in a window that
      // carries the bridge (the IPC sender-origin check must reject it).
      void win.webContents.loadURL(url).catch(() => {});
      return track(win);
    },
    closeAll() {
      for (const win of windows.values()) if (!win.isDestroyed()) win.destroy();
    },
    enginePort: deps.enginePort,
    captureOpenExternal: () => deps.captureOpenExternal(opened),
    openedExternal: () => [...opened],
  };
}
