// SPDX-License-Identifier: GPL-3.0-or-later
// Detached panels (ADR-0031, WP-2H.14): the one `window.open` main allows. An editor window may
// open `popout.html` on the app origin under dockview's window name (`<dock>-<group>`); the
// window it gets is a `popout` (window-policy.ts): the editor's process, no preload, no Node, the
// app's CSP. Everything else stays denied (hardening.ts). Pure and unit-tested; the Electron
// wiring is the window-open handler in hardening.ts and `did-create-window` in index.ts.

import type { BrowserWindowConstructorOptions } from "electron";
import { isTrustedOrigin, type OriginPolicy } from "./origins.ts";
import { webPreferencesFor, type WindowPolicyContext } from "./window-policy.ts";
import { type Bounds, fitToDisplays } from "./window-state.ts";

/** The one page a popout loads: a blank document with no script, which the editor fills. */
export const POPOUT_PATH = "/popout.html";

/**
 * dockview's name for a popout window, `<dock id>-<group id>`: both are its own counters. The
 * Keep on top channel names a popout by it (`window:setPopoutOnTop`).
 */
export const POPOUT_FRAME_NAME = /^\d{1,9}-\d{1,9}$/;

/** A popout is never smaller than this, so a restored one stays usable. */
export const POPOUT_MIN = { width: 240, height: 160 } as const;

export interface PopoutRequest {
  readonly url: string;
  readonly frameName: string;
  /** The window that called `window.open` was made by the factory as an `editor` window. */
  readonly openerIsEditor: boolean;
}

function parse(raw: string): URL | null {
  try {
    return new URL(raw);
  } catch {
    return null;
  }
}

/** `app://cutweave/popout.html` (dev: the dev server's), with no query and no fragment. */
export function isPopoutUrl(raw: string, policy: OriginPolicy): boolean {
  if (!isTrustedOrigin(raw, policy)) return false;
  const url = parse(raw);
  return url !== null && url.pathname === POPOUT_PATH && url.search === "" && url.hash === "";
}

/** Why a `window.open` is not a popout main allows; `null` when it is one. */
export function popoutRefusal(request: PopoutRequest, policy: OriginPolicy): string | null {
  if (!request.openerIsEditor) return "not opened by an editor window";
  if (!isPopoutUrl(request.url, policy)) return "not the popout page";
  if (!POPOUT_FRAME_NAME.test(request.frameName)) return "not a dock's popout window name";
  return null;
}

/**
 * What is wrong with a created popout's live webPreferences (`getLastWebPreferences()`), or
 * `null`: it must run in the editor's process (`sandbox: false`) with no preload and no Node. The
 * second check after the options main gave it; a window that fails it is destroyed.
 */
export function livePopoutProblem(
  prefs: {
    readonly preload?: unknown;
    readonly sandbox?: unknown;
    readonly nodeIntegration?: unknown;
    readonly nodeIntegrationInSubFrames?: unknown;
    readonly contextIsolation?: unknown;
  } | null,
): string | null {
  if (prefs === null) return "it has no webPreferences";
  if (typeof prefs.preload === "string" && prefs.preload !== "") return "it has a preload";
  if (prefs.sandbox !== false) return "it is not in its editor's process (sandbox)";
  if (prefs.nodeIntegration === true || prefs.nodeIntegrationInSubFrames === true) {
    return "it has Node integration";
  }
  if (prefs.contextIsolation === false) return "it has no context isolation";
  return null;
}

/** A popout's top frame loads the popout page and never navigates anywhere else. */
export function isAllowedPopoutNavigation(raw: string, policy: OriginPolicy): boolean {
  return isPopoutUrl(raw, policy);
}

/**
 * The options main gives an allowed popout: the policy's webPreferences (window-policy.ts), the
 * native frame with no menu bar, and the theme's frame color so it never flashes white. Its size
 * is the content's (dockview saves `innerWidth`/`innerHeight`), so a reopened window keeps it.
 */
export function popoutWindowOptions(
  ctx: WindowPolicyContext,
  backgroundColor: string,
): BrowserWindowConstructorOptions {
  return {
    autoHideMenuBar: true,
    useContentSize: true,
    minWidth: POPOUT_MIN.width,
    minHeight: POPOUT_MIN.height,
    backgroundColor,
    webPreferences: webPreferencesFor("popout", ctx),
  };
}

/**
 * Where a popout opens: the bounds it asked for, at least {@link POPOUT_MIN} and no larger than
 * the largest display, on a connected display. A window whose display is gone loses its position
 * (`x`/`y` absent: the caller centers it).
 */
export function fitPopout(
  requested: Bounds,
  displays: readonly Bounds[],
): Partial<Bounds> & Pick<Bounds, "width" | "height"> {
  const maxWidth = Math.max(POPOUT_MIN.width, ...displays.map((d) => d.width));
  const maxHeight = Math.max(POPOUT_MIN.height, ...displays.map((d) => d.height));
  const capped = {
    ...requested,
    width: Math.min(requested.width, maxWidth),
    height: Math.min(requested.height, maxHeight),
  };
  return fitToDisplays({ bounds: capped, maximized: false }, displays, POPOUT_MIN).bounds;
}
