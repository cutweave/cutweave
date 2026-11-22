// SPDX-License-Identifier: GPL-3.0-or-later
// Loads the `cutweave-bridge` napi addon (WP-1H.03). ADR-001 transport (a): the addon is loaded
// here, in the main editor window's non-sandboxed preload only, and reaches the page solely
// through the narrow `window.cutweave.preview` API (api.ts). Nothing in this file is exposed to
// the page directly.

import path from "node:path";
import type * as Native from "../../native/index.d.ts";
import type {
  PreviewClock,
  PreviewFrame,
  PreviewRingInfo,
  PreviewStatus,
  PreviewWaitResult,
} from "../shared/api.ts";

/** What the preload needs from the addon: one open ring at a time. */
export interface BridgeAddon {
  /** Open (replacing any open ring). Throws with an `ERR_RING_*` code in the message. */
  open(ringName: string, epoch: string): PreviewRingInfo;
  getLatestFrame(maxPtsFlicks?: number): PreviewFrame | null;
  /** Clock-scheduled choice: newest not-yet-returned frame with `pts <= clock - latencyMs`. */
  frameAtClock(latencyMs: number): PreviewFrame | null;
  waitForFrame(timeoutMs: number): Promise<PreviewWaitResult>;
  getClock(): PreviewClock;
  getStatus(): PreviewStatus;
  close(): void;
}

export const ERR_RING_NOT_OPEN = "ERR_RING_CLOSED";

/** Where the build (`scripts/build-bridge.ts`) stages the addon, relative to `dist/preload/`. */
export function defaultAddonPath(dirname: string): string {
  return path.join(dirname, "..", "..", "native", "cutweave_bridge.node");
}

/** Adapt the napi module to [`BridgeAddon`]: one `Ring` at a time, errors for a closed ring. */
export function createBridgeAddon(native: typeof Native): BridgeAddon {
  let ring: Native.Ring | null = null;
  const current = (): Native.Ring => {
    if (ring === null) throw new Error(`${ERR_RING_NOT_OPEN}: no preview ring is open`);
    return ring;
  };
  return {
    open(ringName, epoch) {
      ring?.close();
      ring = null;
      const opened = native.openRing(ringName, epoch);
      ring = opened;
      const info = opened.header();
      return {
        name: info.name,
        epoch: info.epoch,
        writerPid: info.writerPid,
        slotCount: info.slotCount,
        slotCapacity: info.slotCapacity,
      };
    },
    getLatestFrame: (maxPtsFlicks) => current().latestFrame(maxPtsFlicks ?? null),
    frameAtClock: (latencyMs) => current().frameAtClock(latencyMs),
    waitForFrame: (timeoutMs) => current().waitForFrame(timeoutMs),
    getClock() {
      const clock = current().audioClock();
      if (clock === null) return { ptsFlicks: null };
      return { ptsFlicks: clock.ptsFlicks, state: clock.state, rateMilli: clock.rateMilli };
    },
    getStatus: () => current().status(),
    close() {
      ring?.close();
      ring = null;
    },
  };
}

export interface LoadOptions {
  /** Absolute path of the `.node` file. Default: `defaultAddonPath(__dirname)`. */
  path?: string;
  /** Module loader. Default: the preload's CommonJS `require`. */
  load?: (path: string) => unknown;
  /** Where load failures are reported. Default: `console.error`. */
  report?: (message: string) => void;
}

/**
 * Try to load the addon. Returns null when the binary is missing or fails to load (wrong
 * platform, ABI mismatch, a policy block); the preview API then throws `ERR_BRIDGE_UNAVAILABLE`
 * and the renderer falls back to JPEG frames over the engine's `/media/frame`.
 */
export function loadBridgeAddon(options: LoadOptions = {}): BridgeAddon | null {
  const report = options.report ?? ((m: string) => console.error(`[bridge] ${m}`));
  try {
    const file = options.path ?? defaultAddonPath(__dirname);
    const load = options.load ?? require;
    return createBridgeAddon(load(file) as typeof Native);
  } catch (err) {
    report(`cutweave-bridge addon not loaded: ${err instanceof Error ? err.message : String(err)}`);
    return null;
  }
}
