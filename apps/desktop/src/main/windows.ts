// SPDX-License-Identifier: GPL-3.0-or-later
// `createWindow(kind)`: the one place that constructs a BrowserWindow (a unit test enforces it).
// It applies the per-kind webPreferences policy (window-policy.ts), refuses any URL outside the
// app origin and records each window's kind for the IPC sender check.

import {
  BrowserWindow,
  type BrowserWindowConstructorOptions,
  type WebContents,
  type WebPreferences,
} from "electron";
import { isTrustedOrigin, type OriginPolicy } from "./origins.ts";
import {
  resolveWebPreferences,
  type WindowKind,
  type WindowPolicyContext,
  WindowPolicyError,
} from "./window-policy.ts";

export interface WindowFactoryContext extends WindowPolicyContext, OriginPolicy {}

export interface CreateWindowOptions
  extends Omit<BrowserWindowConstructorOptions, "webPreferences"> {
  /** Must be on the app origin (or the dev server in dev). */
  url: string;
  /** Only TUNABLE keys or values equal to the policy; see window-policy.ts. */
  webPreferences?: WebPreferences;
}

const kinds = new Map<number, WindowKind>();

/** Each popout window (by its webContents id): the editor that opened it and dockview's name. */
const popouts = new Map<number, { readonly opener: number; readonly frameName: string }>();

/** Kind of the window owning `contents`, if the factory (or `registerPopout`) recorded it. */
export function windowKindOf(contents: unknown): WindowKind | undefined {
  if (typeof contents !== "object" || contents === null || !("id" in contents)) return undefined;
  const { id } = contents as WebContents;
  return typeof id === "number" ? kinds.get(id) : undefined;
}

/**
 * Records a popout (ADR-0031): a window Electron created for an editor window's allowed
 * `window.open` (popout.ts), never through `createWindow`. Main checked its policy first.
 */
export function registerPopout(child: BrowserWindow, opener: WebContents, frameName: string): void {
  const id = child.webContents.id;
  kinds.set(id, "popout");
  popouts.set(id, { opener: opener.id, frameName });
  child.webContents.once("destroyed", () => {
    kinds.delete(id);
    popouts.delete(id);
  });
}

/** The live popouts `opener` opened. */
export function popoutsOf(opener: WebContents): BrowserWindow[] {
  return windowsOfKind("popout").filter(
    (win) => popouts.get(win.webContents.id)?.opener === opener.id,
  );
}

/** The popout `opener` opened under dockview's window name `frameName`, if it is open. */
export function popoutNamed(opener: WebContents, frameName: string): BrowserWindow | undefined {
  return popoutsOf(opener).find((win) => popouts.get(win.webContents.id)?.frameName === frameName);
}

/** Every live window the factory created as `kind`. */
export function windowsOfKind(kind: WindowKind): BrowserWindow[] {
  return BrowserWindow.getAllWindows().filter(
    (win) => !win.isDestroyed() && kinds.get(win.webContents.id) === kind,
  );
}

export function createWindow(
  kind: WindowKind,
  options: CreateWindowOptions,
  ctx: WindowFactoryContext,
): BrowserWindow {
  const { url, webPreferences, ...rest } = options;
  // A popout exists only as an editor window's `window.open` (hardening.ts, popout.ts).
  if (kind === "popout") {
    throw new WindowPolicyError("popout window: only an editor window opens one (window.open)");
  }
  const prefs = resolveWebPreferences(kind, ctx, webPreferences);
  if (!isTrustedOrigin(url, ctx)) {
    throw new WindowPolicyError(`${kind} window: may only load the app origin`);
  }

  const win = new BrowserWindow({ ...rest, webPreferences: prefs });
  const id = win.webContents.id;
  kinds.set(id, kind);
  win.webContents.once("destroyed", () => kinds.delete(id));
  win.loadURL(url).catch((err: unknown) => {
    // A reload (e.g. after an engine port change) aborts the pending first load; not an error.
    if ((err as { code?: unknown }).code === "ERR_ABORTED") return;
    console.error(`${kind} window failed to load:`, err instanceof Error ? err.message : err);
  });
  return win;
}
