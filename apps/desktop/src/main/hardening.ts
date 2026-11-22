// SPDX-License-Identifier: GPL-3.0-or-later
// Electron wiring of the WP-1L.02 controls (arch §18): per-webContents navigation, window-open and
// webview guards, and per-session permission, device, download and request policies. The
// decisions themselves are pure functions in origins.ts / csp.ts (unit-tested).

import type { Session, WebContents } from "electron";
import { buildCsp } from "./csp.ts";
import {
  isAllowedFrameNavigation,
  isAllowedNavigation,
  isAllowedRequest,
  isTrustedOrigin,
  type OriginPolicy,
  type RequestPolicy,
} from "./origins.ts";

/**
 * Permissions the editor needs, granted to trusted origins only. Empty: nothing in Phase 1 needs
 * one. Adding an entry is a security review item (SECURITY-MODEL.md).
 */
export const ALLOWED_PERMISSIONS: readonly string[] = [];

export function isPermissionAllowed(
  permission: string,
  requestingUrl: string,
  policy: OriginPolicy,
): boolean {
  return ALLOWED_PERMISSIONS.includes(permission) && isTrustedOrigin(requestingUrl, policy);
}

export interface HardeningContext extends OriginPolicy {
  readonly isDev: boolean;
  /** Current engine port (null when engine-less); read on every request. */
  enginePort(): number | null;
  /** Diagnostics sink for blocked actions (main's console by default). */
  log?(message: string): void;
}

const hardenedSessions = new WeakSet<Session>();

function logger(ctx: HardeningContext): (message: string) => void {
  return ctx.log ?? ((message) => console.warn(`[security] ${message}`));
}

/** Attach the guards to every webContents (`app.on("web-contents-created")`). */
export function hardenWebContents(contents: WebContents, ctx: HardeningContext): void {
  const log = logger(ctx);

  // window.open / target=_blank: always denied (returns null in the page). External links go
  // through the `shell:openExternal` IPC only.
  contents.setWindowOpenHandler(({ url }) => {
    log(`window.open denied: ${url}`);
    return { action: "deny" };
  });

  contents.on("will-frame-navigate", (event) => {
    const allowed = event.isMainFrame
      ? isAllowedNavigation(event.url, ctx)
      : isAllowedFrameNavigation(event.url, ctx);
    if (!allowed) {
      event.preventDefault();
      log(`${event.isMainFrame ? "navigation" : "frame navigation"} blocked: ${event.url}`);
    }
  });
  // Older path for top-level navigations; same decision.
  contents.on("will-navigate", (event) => {
    if (!isAllowedNavigation(event.url, ctx)) event.preventDefault();
  });
  contents.on("will-redirect", (event) => {
    const allowed = event.isMainFrame
      ? isAllowedNavigation(event.url, ctx)
      : isAllowedFrameNavigation(event.url, ctx);
    if (!allowed) {
      event.preventDefault();
      log(`redirect blocked: ${event.url}`);
    }
  });

  // <webview> is disabled by webPreferences; refuse attachment anyway.
  contents.on("will-attach-webview", (event) => {
    event.preventDefault();
    log("webview attachment blocked");
  });
  contents.on("select-bluetooth-device", (event, _devices, callback) => {
    event.preventDefault();
    callback("");
  });

  if (!ctx.isDev) {
    contents.on("devtools-opened", () => contents.closeDevTools());
  }
}

/**
 * Harden a session (default session at `ready`, every later one via `app.on("session-created")`).
 * Idempotent.
 */
export function hardenSession(ses: Session, ctx: HardeningContext): void {
  if (hardenedSessions.has(ses)) return;
  hardenedSessions.add(ses);
  const log = logger(ctx);

  ses.setPermissionRequestHandler((_contents, permission, callback, details) => {
    const allowed = isPermissionAllowed(permission, details.requestingUrl, ctx);
    if (!allowed) log(`permission denied: ${permission}`);
    callback(allowed);
  });
  ses.setPermissionCheckHandler((_contents, permission, requestingOrigin) =>
    isPermissionAllowed(permission, requestingOrigin, ctx),
  );
  ses.setDevicePermissionHandler(() => false);
  ses.on("select-hid-device", (event, _details, callback) => {
    event.preventDefault();
    callback(null);
  });
  ses.on("select-serial-port", (event, _ports, _contents, callback) => {
    event.preventDefault();
    callback("");
  });
  ses.on("select-usb-device", (event, _details, callback) => {
    event.preventDefault();
    callback();
  });
  if (process.platform !== "darwin") {
    ses.setBluetoothPairingHandler((_details, callback) => callback({ confirmed: false }));
  }

  // Exports are written by the engine; a page never saves files through Chromium downloads.
  ses.on("will-download", (event) => {
    event.preventDefault();
    log("download blocked");
  });

  // No remote content: a second layer under the CSP that also covers requests the CSP does not
  // see (navigations, workers). Main-process requests have no webContents.
  ses.webRequest.onBeforeRequest((details, callback) => {
    const policy: RequestPolicy = {
      devServerOrigin: ctx.devServerOrigin,
      enginePort: ctx.enginePort(),
      isDev: ctx.isDev,
    };
    const allowed = isAllowedRequest(details.url, policy, details.webContentsId === undefined);
    if (!allowed) log(`request blocked: ${details.resourceType} ${details.url}`);
    callback({ cancel: !allowed });
  });

  // Dev only: documents from the Vite dev server do not pass through the app:// handler, so they
  // get the (dev-relaxed) CSP here.
  if (ctx.devServerOrigin !== undefined) {
    const devOrigin = ctx.devServerOrigin;
    ses.webRequest.onHeadersReceived((details, callback) => {
      if (!details.url.startsWith(`${devOrigin}/`)) {
        callback({});
        return;
      }
      const headers = { ...details.responseHeaders };
      for (const name of Object.keys(headers)) {
        if (name.toLowerCase() === "content-security-policy") delete headers[name];
      }
      headers["Content-Security-Policy"] = [
        buildCsp(ctx.enginePort(), { devServerOrigin: devOrigin }),
      ];
      callback({ responseHeaders: headers });
    });
  }
}
