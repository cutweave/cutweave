// SPDX-License-Identifier: GPL-3.0-or-later
// Typings of the `cutweave-bridge` napi addon (cutweave_bridge.node, built by
// `scripts/build-bridge.ts` from crates/cutweave-bridge). Maintained by hand: keep it in step with
// the `#[napi]` items of crates/cutweave-bridge/src/node.rs. Only the main window's preload loads
// the addon; the page never sees these types' values except through `window.cutweave.preview`.

export interface RingInfo {
  name: string;
  /** `0x` + 16 hex digits. */
  epoch: string;
  writerPid: number;
  layoutVersion: number;
  slotCount: number;
  slotCapacity: number;
  mappingSize: number;
}

export interface RingStatus {
  /** `live`, `starting` (writer not live yet) or `ended` (ring released; see `reason`). */
  state: "live" | "starting" | "ended";
  alive: boolean;
  reason?: "writer-closed" | "heartbeat-stale" | "successor-gone" | "successor-rejected" | "closed";
  /** `ERR_RING_*` code of a rejected successor. */
  reasonCode?: string;
  name?: string;
  heartbeatAgeMs?: number;
  writeIndex: number;
  lastSeq: number;
  switches: number;
  tornRetries: number;
  framesDropped: number;
  configGen: number;
  previewWidth: number;
  previewHeight: number;
}

export interface PreviewFrame {
  pts: number;
  duration: number;
  seq: number;
  width: number;
  height: number;
  format: "rgba8" | "nv12";
  /** Payload as published (rows may be padded); pooled: valid until the second next call. */
  bytes: ArrayBuffer;
  stride: number;
  plane1Offset: number;
  plane1Stride: number;
  lowres: boolean;
  rev: number;
  configGen: number;
  renderAgeMs: number;
  publishAgeMs: number;
}

export interface AudioClock {
  ptsFlicks: number;
  state: "stopped" | "playing" | "scrubbing";
  rateMilli: number;
  ageMs: number;
}

export interface PerfCounters {
  ringsOpened: number;
  framesRead: number;
  bytesCopied: number;
  tornRetries: number;
  successorSwitches: number;
}

export type WaitOutcome = "frame" | "changed" | "ended" | "timeout";

export declare class Ring {
  header(): RingInfo;
  status(): RingStatus;
  latestFrame(maxPtsFlicks?: number | null): PreviewFrame | null;
  frameAtClock(latencyMs?: number | null): PreviewFrame | null;
  waitForFrame(timeoutMs: number): Promise<WaitOutcome>;
  audioClock(): AudioClock | null;
  close(): void;
}

/** Throws errors whose message starts with an `ERR_RING_*` code. */
export declare function openRing(name: string, epoch: bigint | string): Ring;
export declare function perfCounters(): PerfCounters;
