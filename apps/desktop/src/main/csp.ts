// SPDX-License-Identifier: GPL-3.0-or-later
// Content-Security-Policy for the editor (arch §6, §18; WP-1L.02). Main serves it as a response
// header from the `app://` protocol handler, built per response from the engine port that main
// chose at launch, so a re-picked port (engine exit 12) only needs a renderer reload.

import { PLUGIN_SCHEME } from "./app-protocol.ts";

export interface CspOptions {
  /**
   * Dev only (`just dev`): the Vite dev server origin. Adds it to the fetch directives and allows
   * inline scripts/styles that Vite's HMR and React refresh inject. Never set when packaged.
   */
  readonly devServerOrigin?: string | undefined;
}

/** Throws unless `port` is a valid TCP port. */
function checkPort(port: number): void {
  if (!Number.isInteger(port) || port < 1 || port > 65_535) {
    throw new RangeError(`invalid engine port: ${port}`);
  }
}

/**
 * Build the policy. `enginePort` null (engine-less run) leaves `connect-src 'none'`.
 *
 * - `default-src 'none'`, then only what the editor needs: scripts, styles and fonts from the app
 *   origin; images and media also from `data:`/`blob:` and the engine's `/media` HTTP routes.
 * - `connect-src`: the engine origin only (`http://127.0.0.1:P` and `ws://127.0.0.1:P`).
 * - No `'unsafe-eval'`, no `'unsafe-inline'`, no `'wasm-unsafe-eval'` in production.
 * - `frame-src`: the dedicated plugin scheme only; `object-src`, `base-uri`, `form-action`,
 *   `frame-ancestors`: none.
 */
export function buildCsp(enginePort: number | null, options: CspOptions = {}): string {
  const engine: string[] = [];
  const engineWs: string[] = [];
  if (enginePort !== null) {
    checkPort(enginePort);
    engine.push(`http://127.0.0.1:${enginePort}`);
    engineWs.push(`ws://127.0.0.1:${enginePort}`);
  }

  const dev: string[] = [];
  const devWs: string[] = [];
  const devInline: string[] = [];
  if (options.devServerOrigin !== undefined) {
    const origin = new URL(options.devServerOrigin);
    dev.push(origin.origin);
    devWs.push(`ws://${origin.host}`);
    devInline.push("'unsafe-inline'");
  }

  const connect = [...engine, ...engineWs, ...dev, ...devWs];
  const directives: [string, string[]][] = [
    ["default-src", ["'none'"]],
    ["script-src", ["'self'", ...dev, ...devInline]],
    ["style-src", ["'self'", ...dev, ...devInline]],
    ["img-src", ["'self'", "data:", "blob:", ...engine, ...dev]],
    ["font-src", ["'self'", ...dev]],
    ["connect-src", connect.length > 0 ? connect : ["'none'"]],
    ["media-src", ["blob:", ...engine]],
    ["frame-src", [`${PLUGIN_SCHEME}:`]],
    ["object-src", ["'none'"]],
    ["base-uri", ["'none'"]],
    ["form-action", ["'none'"]],
    ["frame-ancestors", ["'none'"]],
  ];
  return directives.map(([name, sources]) => `${name} ${sources.join(" ")}`).join("; ");
}

/** Every header main sets on `app://` responses, given the CSP for the current engine port. */
export function appResponseHeaders(csp: string): Record<string, string> {
  return {
    "Content-Security-Policy": csp,
    "X-Content-Type-Options": "nosniff",
    "Cross-Origin-Opener-Policy": "same-origin",
    "Referrer-Policy": "no-referrer",
    "Cache-Control": "no-cache",
  };
}
