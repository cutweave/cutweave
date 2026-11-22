// SPDX-License-Identifier: GPL-3.0-or-later
// Transport (b) (WP-0C.06) message contracts:
//   main <-> utility   over the utilityProcess channel (parentPort / child.postMessage)
//   utility <-> renderer over a MessageChannelMain whose renderer end is handed to the
//                      sandboxed window by its preload (the preload's ONLY job).
// The UI token stays in main and the utility process; it never reaches the renderer.

import type { LatSummary } from "../shared/loop.ts";
import type { EngineEvent, EngineInfo, FrameHeader } from "../shared/types.ts";

/** Engine audio clock as sampled by the utility right before it posts a frame. */
export interface ClockSample {
  flicks: number;
  /** Host µs (Unix epoch) at which `flicks` was valid. */
  hostUs: number;
  /** 0 starting, 1 playing, 2 paused, 3 stopped. */
  state: number;
  engineDropped: number;
}

export type MainToUtility =
  | { type: "engine-up"; info: EngineInfo; token: string }
  | { type: "engine-down"; ev: EngineEvent & { state: "down" } }
  /** ports[0] is the utility end of a fresh channel to the renderer. */
  | { type: "renderer-port" }
  | { type: "ring-exists"; id: number; ring: string }
  | { type: "stats"; id: number; reset: boolean };

export interface UtilityProbes {
  addonLoaded: boolean;
  node: string;
  /** `RingReader.latestView().zeroCopy` in the utility (Electron's V8 cage: expected false). */
  externalArrayBuffers: boolean | null;
  /** Result of `port.postMessage(msg, [arrayBuffer])` (MessagePortMain only transfers ports). */
  arrayBufferTransfer: string | null;
  /** Result of posting a SharedArrayBuffer to the renderer (cannot cross processes). */
  sharedArrayBuffer: string | null;
  /** Control WebSocket from the utility (Node/undici client) accepted by the engine. */
  controlSocket: string | null;
}

export interface UtilityStats {
  framesSent: number;
  bytesSent: number;
  /** Frames skipped because `maxInFlight` unacknowledged frames were already queued. */
  creditDrops: number;
  /** Slot overwritten between header snapshot and copy, or torn after retries. */
  readErrors: number;
  wakeups: number;
  timeouts: number;
  maxInFlight: number;
  /** shm -> utility JS heap (napi, seqlock-validated): copy 1. */
  copyMs: LatSummary;
  /** `MessagePortMain.postMessage` wall time: V8 serialization + Mojo BigBuffer (copies 2-3). */
  postMs: LatSummary;
  /** ring publish -> utility wake-up (poll thread + libuv async). */
  publishToWakeMs: LatSummary;
}

export type UtilityToMain =
  | { type: "ready"; probes: UtilityProbes }
  | { type: "probes"; probes: UtilityProbes }
  | { type: "ring-closed"; ring: string }
  | { type: "ring-exists"; id: number; exists: boolean }
  | { type: "stats"; id: number; stats: UtilityStats };

export type ControlMsg =
  | { type: "play" }
  | { type: "pause" }
  | { type: "seek"; pts: number }
  | { type: "format"; format: "rgba8" | "nv12" }
  | { type: "ping"; t: number };

/** A frame message. `pixels` is structured-cloned by Electron (it cannot be transferred). */
export interface FrameMsg {
  type: "frame";
  h: FrameHeader;
  clock: ClockSample;
  /** Host µs right before `postMessage`. */
  tSendUs: number;
  /** Duration of the previous `postMessage` call (µs). */
  prevPostUs: number;
  pixels: ArrayBuffer;
}

export type UtilityToRenderer =
  | FrameMsg
  | { type: "engine"; ev: EngineEvent }
  | { type: "sync"; t0: number; hostUs: number }
  | { type: "pong"; t: number }
  | { type: "probe" };

export type RendererToUtility =
  | { type: "ack"; seq: number }
  | { type: "sync"; t0: number }
  | { type: "control"; msg: ControlMsg }
  | { type: "config"; maxInFlight?: number };

/** Strings exchanged between the sandboxed preload and the page over `window.postMessage`. */
export const WANT_PORT = "s1b:want-port";
export const PORT_DELIVERY = "s1b:port";
