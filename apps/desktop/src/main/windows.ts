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

/** Kind of the window owning `contents`, if the factory created it. */
export function windowKindOf(contents: unknown): WindowKind | undefined {
  if (typeof contents !== "object" || contents === null || !("id" in contents)) return undefined;
  const { id } = contents as WebContents;
  return typeof id === "number" ? kinds.get(id) : undefined;
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
