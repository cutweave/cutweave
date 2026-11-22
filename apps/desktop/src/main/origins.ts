// SPDX-License-Identifier: GPL-3.0-or-later
// Pure URL policy (arch §18, WP-1L.02): which origins are trusted, where windows and frames may
// navigate, which requests a renderer may make and which links may open in the OS browser.
// Electron wiring lives in hardening.ts; everything here is unit-tested without Electron.

import { APP_HOST, APP_SCHEME, PLUGIN_SCHEME } from "./app-protocol.ts";

export interface OriginPolicy {
  /**
   * Dev only: origin of the Vite dev server (`http://localhost:5173`). Never set when packaged;
   * main derives it from `CUTWEAVE_DEV_URL` only for unpackaged runs.
   */
  readonly devServerOrigin?: string | undefined;
}

export interface RequestPolicy extends OriginPolicy {
  /** Port of the supervised engine, or null while the app runs engine-less. */
  readonly enginePort: number | null;
  /** DevTools may load its own frontend in dev builds only. */
  readonly isDev: boolean;
}

/** Longest URL `shell:openExternal` accepts. */
export const MAX_EXTERNAL_URL_LENGTH = 2048;

function parse(raw: string): URL | null {
  try {
    return new URL(raw);
  } catch {
    return null;
  }
}

/**
 * True for `app://cutweave/...`. Compares scheme and host instead of `URL.origin`, which is
 * opaque ("null") for custom schemes in Node, and rejects userinfo (`app://cutweave@evil/`).
 */
export function isAppUrl(raw: string): boolean {
  const url = parse(raw);
  return (
    url !== null &&
    url.protocol === `${APP_SCHEME}:` &&
    url.host === APP_HOST &&
    url.username === "" &&
    url.password === ""
  );
}

function isDevServerUrl(raw: string, policy: OriginPolicy): boolean {
  if (policy.devServerOrigin === undefined) return false;
  const url = parse(raw);
  const dev = parse(policy.devServerOrigin);
  return url !== null && dev !== null && url.origin === dev.origin && url.username === "";
}

/** Origins whose documents may host the bridge and call IPC: the app origin (and the dev server). */
export function isTrustedOrigin(raw: string, policy: OriginPolicy): boolean {
  return isAppUrl(raw) || isDevServerUrl(raw, policy);
}

/** Top-level navigation: the app origin only (plus the dev server in dev). */
export function isAllowedNavigation(raw: string, policy: OriginPolicy): boolean {
  return isTrustedOrigin(raw, policy);
}

/**
 * Subframe navigation: the app origin, inert `about:blank`/`about:srcdoc` documents and the
 * dedicated plugin scheme (plugin iframes, P6). Never `file:`, `http(s):` or `data:`.
 */
export function isAllowedFrameNavigation(raw: string, policy: OriginPolicy): boolean {
  if (raw === "about:blank" || raw === "about:srcdoc") return true;
  const url = parse(raw);
  if (url !== null && url.protocol === `${PLUGIN_SCHEME}:`) return true;
  return isTrustedOrigin(raw, policy);
}

/**
 * Validate a link for `shell.openExternal`: `https:` only, a host, no credentials, bounded length.
 * Returns the normalized URL or null.
 */
export function parseExternalUrl(raw: string): string | null {
  if (raw.length === 0 || raw.length > MAX_EXTERNAL_URL_LENGTH) return null;
  const url = parse(raw);
  if (url === null || url.protocol !== "https:") return null;
  if (url.hostname === "" || url.username !== "" || url.password !== "") return null;
  return url.toString();
}

const LOOPBACK = "127.0.0.1";

/**
 * Session-level request filter ("no remote content"), a second layer under the CSP. Renderers
 * may load the app origin, the engine's loopback origin and inline `data:`/`blob:` resources;
 * `file:` is allowed only for main's own `app://` protocol handler (no renderer, `fromMain`).
 * Everything else (remote `http(s)`/`ws(s)`, `file:` from a page, extensions) is cancelled.
 */
export function isAllowedRequest(raw: string, policy: RequestPolicy, fromMain: boolean): boolean {
  const url = parse(raw);
  if (url === null) return false;
  switch (url.protocol) {
    case `${APP_SCHEME}:`:
      return isAppUrl(raw);
    case "data:":
    case "blob:":
      return true;
    case "file:":
      return fromMain;
    case "devtools:":
      return policy.isDev;
    case "http:":
    case "ws:":
      // Vite's HMR socket is ws:// on the dev server's host and port.
      if (isDevServerUrl(raw.replace(/^ws:/, "http:"), policy)) return true;
      return (
        policy.enginePort !== null &&
        url.hostname === LOOPBACK &&
        url.port === String(policy.enginePort) &&
        url.username === ""
      );
    default:
      return false;
  }
}
