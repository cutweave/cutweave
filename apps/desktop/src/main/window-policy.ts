// SPDX-License-Identifier: GPL-3.0-or-later
// webPreferences policy per window kind (arch §2, §18; ADR-0001; WP-1L.02). Pure and unit-tested;
// `createWindow(kind)` in windows.ts is the only place that constructs a BrowserWindow.
//
// - `editor` (the main editor window): contextIsolation on, nodeIntegration off, and
//   `sandbox: false` ONLY because its preload loads the native frame-bridge addon (ADR-0001
//   transport (a)). It is the only kind with a preload.
// - `popout` (a detached panel's window, ADR-0031): opened only by an editor window, for
//   `popout.html` (popout.ts). It runs in the editor's renderer process, so it is
//   `sandbox: false` like its opener (Electron 45 puts a popup whose sandbox differs from its
//   opener's in another process, where the editor cannot script it), with NO preload and no Node.
//   Its document runs no code of its own: the editor renders into it.
// - every other kind: `sandbox: true`, no preload, no addon.
// Any other combination throws a WindowPolicyError.

import type { WebPreferences } from "electron";

export const WINDOW_KINDS = ["editor", "popout", "auxiliary"] as const;
export type WindowKind = (typeof WINDOW_KINDS)[number];

export interface WindowPolicyContext {
  /** Absolute path of the editor preload bundle (`dist/preload/index.cjs`). */
  readonly preloadPath: string;
  /** Unpackaged run: DevTools allowed. Packaged builds never get DevTools. */
  readonly isDev: boolean;
}

export class WindowPolicyError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "WindowPolicyError";
  }
}

/** Settings every kind shares; a caller may repeat them but never change them. */
const LOCKED = {
  contextIsolation: true,
  nodeIntegration: false,
  nodeIntegrationInWorker: false,
  nodeIntegrationInSubFrames: false,
  webSecurity: true,
  allowRunningInsecureContent: false,
  webviewTag: false,
  experimentalFeatures: false,
  plugins: false,
  navigateOnDragDrop: false,
  safeDialogs: true,
} as const satisfies WebPreferences;

/**
 * Settings a caller may tune per window: none of them widens what the page can reach. Anything
 * not here and not in the policy (`partition`, `session`, `additionalArguments`,
 * `enableBlinkFeatures`, `preload` on a sandboxed kind, …) is rejected.
 */
const TUNABLE = new Set<string>([
  "zoomFactor",
  "backgroundThrottling",
  "defaultFontSize",
  "defaultMonospaceFontSize",
  "minimumFontSize",
  "defaultEncoding",
  "spellcheck",
  "autoplayPolicy",
  "textAreasAreResizable",
  "scrollBounce",
  "disableHtmlFullscreenWindowResize",
  "accessibleTitle",
  "disableDialogs",
  "transparent",
  "enablePreferredSizeMode",
]);

export function isWindowKind(value: unknown): value is WindowKind {
  return typeof value === "string" && (WINDOW_KINDS as readonly string[]).includes(value);
}

/** The policy for `kind`: exactly the security-relevant webPreferences the factory enforces. */
export function webPreferencesFor(kind: WindowKind, ctx: WindowPolicyContext): WebPreferences {
  const base: WebPreferences = { ...LOCKED, devTools: ctx.isDev };
  if (kind === "editor") return { ...base, sandbox: false, preload: ctx.preloadPath };
  if (kind === "popout") return { ...base, sandbox: false };
  return { ...base, sandbox: true };
}

/** Whether `kind` runs unsandboxed: the editor (its addon preload) and its popouts (its process). */
export function isUnsandboxedKind(kind: WindowKind): boolean {
  return kind === "editor" || kind === "popout";
}

/**
 * Check a complete webPreferences object against the policy for `kind`. Throws on the first
 * violation; used by the factory on its own output as a second check.
 */
export function assertWindowPreferences(
  kind: WindowKind,
  prefs: WebPreferences,
  ctx: WindowPolicyContext,
): void {
  const fail = (detail: string): never => {
    throw new WindowPolicyError(`${kind} window: ${detail}`);
  };
  for (const [key, value] of Object.entries(LOCKED)) {
    if ((prefs as Record<string, unknown>)[key] !== value)
      fail(`webPreferences.${key} must be ${value}`);
  }
  if (prefs.devTools !== ctx.isDev) fail(`webPreferences.devTools must be ${ctx.isDev}`);
  if (kind === "editor") {
    if (prefs.sandbox !== false)
      fail("the editor window runs its addon preload with sandbox: false");
    if (prefs.preload !== ctx.preloadPath) fail("the editor window must use the audited preload");
  } else if (kind === "popout") {
    if (prefs.sandbox !== false)
      fail("a popout shares its editor's process, so it must match it (sandbox: false)");
    if (prefs.preload !== undefined) fail("must not have a preload");
  } else {
    if (prefs.sandbox !== true) fail("must be sandboxed (sandbox: true)");
    if (prefs.preload !== undefined) fail("must not have a preload");
  }
  for (const key of ["partition", "session", "additionalArguments", "enableBlinkFeatures"]) {
    if ((prefs as Record<string, unknown>)[key] !== undefined)
      fail(`webPreferences.${key} is not allowed`);
  }
}

/**
 * Merge caller `overrides` into the policy for `kind`. Overrides may only repeat a policy value or
 * set a TUNABLE key; anything else (e.g. `sandbox: false` on an auxiliary window) throws.
 */
export function resolveWebPreferences(
  kind: WindowKind,
  ctx: WindowPolicyContext,
  overrides: WebPreferences = {},
): WebPreferences {
  if (!isWindowKind(kind)) throw new WindowPolicyError(`unknown window kind: ${String(kind)}`);
  const policy = webPreferencesFor(kind, ctx) as Record<string, unknown>;
  for (const [key, value] of Object.entries(overrides)) {
    if (value === undefined) continue;
    if (key in policy) {
      if (policy[key] !== value) {
        throw new WindowPolicyError(
          key === "sandbox" && !isUnsandboxedKind(kind)
            ? `${kind} window: must be sandboxed (sandbox: true)`
            : `${kind} window: webPreferences.${key} must be ${String(policy[key])}`,
        );
      }
    } else if (!TUNABLE.has(key)) {
      throw new WindowPolicyError(`${kind} window: webPreferences.${key} is not allowed`);
    }
  }
  const prefs = { ...overrides, ...policy } as WebPreferences;
  assertWindowPreferences(kind, prefs, ctx);
  return prefs;
}
