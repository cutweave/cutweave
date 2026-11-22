// SPDX-License-Identifier: GPL-3.0-or-later
// Hook point for the `cutweave-bridge` napi addon (WP-1H.03). ADR-001 transport (a): the addon
// is loaded here, in the main editor window's non-sandboxed preload only, and reaches the page
// solely through the narrow `window.cutweave.preview` API.

import type { PreviewClock, PreviewFrame } from "../shared/api.ts";

/** What the preload needs from the addon; WP-1H.03 adapts the napi `Ring` to this shape. */
export interface BridgeAddon {
  open(ringName: string): void;
  getLatestFrame(maxPtsFlicks?: number): PreviewFrame | null;
  getClock(): PreviewClock;
  close(): void;
}

/**
 * Try to load the addon. Returns null while it does not exist (WP-1L.01) or when loading fails;
 * the preview API then throws `ERR_BRIDGE_UNAVAILABLE` and the renderer uses the JPEG fallback.
 * WP-1H.03 replaces the body with `require()` of the prebuilt binary.
 */
export function loadBridgeAddon(): BridgeAddon | null {
  return null;
}
