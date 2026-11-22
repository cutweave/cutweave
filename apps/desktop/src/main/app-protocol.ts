// SPDX-License-Identifier: GPL-3.0-or-later
// `app://cutweave/` serves the built editor from the bundle. Main owns the response headers (the
// strict CSP of WP-1L.02 is added here), and there is no `file://` content.

import path from "node:path";
import { pathToFileURL } from "node:url";

export const APP_SCHEME = "app";
export const APP_HOST = "cutweave";
export const APP_ORIGIN = `${APP_SCHEME}://${APP_HOST}`;

/**
 * Map an `app://cutweave/...` URL to a file under `rootDir`, or null when the request is outside
 * the origin or escapes the root. Extension-less paths fall back to `index.html`.
 */
export function resolveAppRequest(rootDir: string, requestUrl: string): string | null {
  let url: URL;
  try {
    url = new URL(requestUrl);
  } catch {
    return null;
  }
  if (url.protocol !== `${APP_SCHEME}:` || url.host !== APP_HOST) return null;

  let pathname: string;
  try {
    pathname = decodeURIComponent(url.pathname);
  } catch {
    return null;
  }
  if (pathname.includes("\0") || pathname.includes("\\")) return null;

  const relative =
    pathname === "/" || path.posix.extname(pathname) === "" ? "/index.html" : pathname;
  const root = path.resolve(rootDir);
  const target = path.resolve(root, `.${relative}`);
  const rel = path.relative(root, target);
  if (rel === "" || rel.startsWith("..") || path.isAbsolute(rel)) return null;
  return target;
}

/** Response headers main adds to every `app://` response. WP-1L.02 adds `buildCsp(port)` here. */
export function baseResponseHeaders(): Record<string, string> {
  return { "X-Content-Type-Options": "nosniff", "Cache-Control": "no-cache" };
}

/** Minimal slice of Electron's `protocol` and `net` used by the handler (fakes in tests). */
export interface ProtocolLike {
  handle(scheme: string, handler: (request: { url: string }) => Promise<Response> | Response): void;
}
export interface NetLike {
  fetch(url: string, init?: { bypassCustomProtocolHandlers?: boolean }): Promise<Response>;
}

export function registerAppProtocol(
  protocol: ProtocolLike,
  net: NetLike,
  rootDir: string,
  headers: () => Record<string, string> = baseResponseHeaders,
): void {
  protocol.handle(APP_SCHEME, async (request) => {
    const file = resolveAppRequest(rootDir, request.url);
    if (file === null) return new Response("Forbidden", { status: 403 });
    let upstream: Response;
    try {
      upstream = await net.fetch(pathToFileURL(file).toString(), {
        bypassCustomProtocolHandlers: true,
      });
    } catch {
      return new Response("Not found", { status: 404 });
    }
    if (!upstream.ok) return new Response("Not found", { status: 404 });
    const out = new Headers(upstream.headers);
    for (const [name, value] of Object.entries(headers())) out.set(name, value);
    return new Response(upstream.body, { status: upstream.status, headers: out });
  });
}
