// SPDX-License-Identifier: GPL-3.0-or-later
// Main-process hooks for the Playwright suites (e2e/security.spec.ts, e2e/supervisor.spec.ts),
// reachable only from main via Playwright's `electronApp.evaluate` (never from a renderer).
// Installed only for unpackaged runs with CUTWEAVE_TEST_HOOKS=1; `shouldInstallTestHooks` is
// unit-tested.

import type { BrowserWindow, WebPreferences } from "electron";
import type { EngineStatus } from "../shared/api.ts";
import type { EngineFailedChoice } from "./engine/failed-dialog.ts";
import type { EngineSupervisor } from "./engine/supervisor.ts";
import { isWindowKind, type WindowKind } from "./window-policy.ts";

export function shouldInstallTestHooks(
  isPackaged: boolean,
  env: Readonly<Record<string, string | undefined>>,
): boolean {
  return !isPackaged && env.CUTWEAVE_TEST_HOOKS === "1";
}

export interface TestHookDeps {
  /** The factory, bound to main's context (it refuses a `popout`: only `window.open` makes one). */
  createWindow(
    kind: WindowKind,
    options: { url: string; show: boolean; webPreferences?: WebPreferences },
  ): BrowserWindow;
  appUrl: string;
  enginePort(): number | null;
  /** Replace `shell.openExternal` so tests never launch a browser; URLs are pushed to `sink`. */
  captureOpenExternal(sink: string[]): void;
  /** The engine supervisor, once started. */
  engine(): EngineSupervisor | null;
  /** Replace the engine-failed dialog (tests answer it instead of a native message box). */
  captureEngineDialog(ask: (status: EngineStatus) => Promise<EngineFailedChoice>): void;
  /** Whether the editor has told main it has unsaved changes (read-only). */
  windowDirty(): boolean;
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
  /** The supervisor's status (what `engine:status` carries); null before the engine starts. */
  engineStatus(): EngineStatus | null;
  /** Pid of the running engine process, if any. */
  enginePid(): number | null;
  /** SIGKILL the engine (a simulated crash); returns the killed pid, or null. */
  killEngine(): number | null;
  /** Forget past crashes so repeated crash tests stay under the crash budget. */
  resetEngineCrashCount(): void;
  /** The project main would reopen after a crash (what it learned from the engine). */
  engineProject(): string | null;
  /** From now on the engine-failed dialog is recorded instead of shown. */
  captureEngineDialog(): void;
  /** Statuses the engine-failed dialog was asked for since `captureEngineDialog()`. */
  engineDialogs(): EngineStatus[];
  /** Answer the oldest unanswered engine-failed dialog; false when none is open. */
  answerEngineDialog(choice: EngineFailedChoice): boolean;
  /**
   * Whether main knows the editor has unsaved changes, so quitting will ask first. Read-only:
   * tests wait for it before quitting instead of racing the editor's report.
   */
  windowDirty(): boolean;
}

declare global {
  var __cutweaveTest: CutweaveTestHooks | undefined;
}

export function installTestHooks(deps: TestHookDeps): void {
  const windows = new Map<number, BrowserWindow>();
  const opened: string[] = [];
  const dialogs: EngineStatus[] = [];
  const answers: ((choice: EngineFailedChoice) => void)[] = [];
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
    engineStatus: () => deps.engine()?.status ?? null,
    enginePid: () => deps.engine()?.pid ?? null,
    killEngine: () => deps.engine()?.kill() ?? null,
    resetEngineCrashCount: () => deps.engine()?.resetCrashHistory(),
    windowDirty: () => deps.windowDirty(),
    engineProject: () => deps.engine()?.project ?? null,
    captureEngineDialog: () =>
      deps.captureEngineDialog(
        (status) =>
          new Promise((resolve) => {
            dialogs.push(status);
            answers.push(resolve);
          }),
      ),
    engineDialogs: () => [...dialogs],
    answerEngineDialog: (choice) => {
      const answer = answers.shift();
      answer?.(choice);
      return answer !== undefined;
    },
  };
}
