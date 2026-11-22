// SPDX-License-Identifier: GPL-3.0-or-later
// Contract between main, the (non-sandboxed) preload and the renderer main world.

import type { FrameHeader, RingHeader } from "../../../bridge/index.d.ts";
import type { LoopSnapshot } from "./loop.ts";

export type { FrameHeader, LoopSnapshot, RingHeader };

/** Engine facts from the handshake (everything except the token). */
export interface EngineInfo {
  pid: number;
  port: number;
  ring: string;
  width: number;
  height: number;
  fps: number;
  slots: number;
  adapter: string;
  backend: string;
  deviceType: string;
  /** Launch counter (1 = first launch, 2 = first restart, ...). */
  generation: number;
}

/** What main hands the preload over IPC once per engine launch. */
export interface Handshake {
  info: EngineInfo;
  token: string;
}

export type EngineEvent =
  | { state: "up"; info: EngineInfo }
  | {
      state: "down";
      ring: string;
      code: number | null;
      signal: string | null;
      /** ms from main's kill() to the child 'exit' event (only for kills main issued). */
      detectMs: number | null;
    };

/** A frame as seen by the renderer main world. `pixels` crossed contextBridge (a copy). */
export interface Frame extends FrameHeader {
  pixels: ArrayBuffer;
  /** Host µs (engine clock) right after the addon copy finished, in the preload. */
  tFetchUs: number;
}

export type PixelFormatName = "rgba8" | "nv12";

/**
 * "latest": newest frame (latest-wins).
 * "next":   oldest unseen frame, at most 2 behind (naive FIFO).
 * "clock":  newest frame with pts <= audio clock - latency target (arch §10 step 3; a first
 *           cut of the WP-0C.02 scheduler). Falls back to "latest" while paused.
 */
export type FramePolicy = "latest" | "next" | "clock";

/** Control surface of the present loop running inside the preload (draw=preload mode). */
export interface PreloadLoopHandle {
  snapshot(recentN?: number): LoopSnapshot | null;
  reset(): void;
  setVerify(on: boolean): void;
  framesPresented(): number;
  framesSinceUp(): number;
}

/** The narrow API exposed as `window.bridge` in the main editor window only. */
export interface S1Bridge {
  readonly version: 1;
  /** Newest frame with seq > `newerThanSeq`, or null. Pixels are cloned across contextBridge. */
  getLatestFrame(newerThanSeq: number): Frame | null;
  /** Frame chosen by `policy` with seq > `afterSeq`, or null. Pixels are cloned. */
  getFrame(afterSeq: number, policy: FramePolicy): Frame | null;
  /** Latency target of the "clock" policy (default 20 ms). */
  setClockLatencyMs(ms: number): void;
  /** Experiment: run the whole present loop in the preload's isolated world, drawing into
   * `canvas` (an Element crosses contextBridge by reference), so pixels never get cloned. */
  presentInPreload(
    canvas: HTMLCanvasElement,
    opts: { policy: FramePolicy; verify: boolean; preferGpu: boolean },
  ): Promise<string>;
  preloadLoop: PreloadLoopHandle;
  readHeader(): RingHeader | null;
  /** Engine-compatible host clock (µs since the Unix epoch). */
  nowUs(): number;
  info(): EngineInfo | null;
  stats(): {
    frames: number;
    torn: number;
    nothingNew: number;
    errors: number;
    /** Whether the addon could alias shared memory as an ArrayBuffer here (null = unknown). */
    externalArrayBuffers: boolean | null;
  };
  transport: {
    play(): void;
    pause(): void;
    seek(ptsFlicks: number): void;
    setFormat(format: PixelFormatName): void;
    /** Round trip over the control WebSocket, in ms. */
    ping(): Promise<number>;
  };
  onEngineEvent(cb: (e: EngineEvent) => void): void;
}

export const FLICKS_PER_SECOND = 705_600_000;
export const FORMAT_RGBA8 = 1;
export const FORMAT_NV12 = 2;
