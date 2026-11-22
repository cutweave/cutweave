// SPDX-License-Identifier: GPL-3.0-or-later
// Hand-written types for s1_bridge.node (napi-rs v3 addon, bridge/src/lib.rs).
// All 64-bit values are JS numbers (exact below 2^53).

export interface FrameHeader {
  slot: number;
  seq: number;
  /** Presentation timestamp in flicks. */
  pts: number;
  width: number;
  height: number;
  /** 1 = RGBA8, 2 = NV12. */
  format: number;
  stride: number;
  rev: number;
  payloadLen: number;
  checksum: number;
  /** Host µs (Unix epoch) at GPU submit in the engine. */
  tRenderUs: number;
  /** Host µs at slot publish. */
  tPublishUs: number;
  lock: number;
  /** µs the addon spent copying this frame (0 for header-only reads). */
  readUs: number;
}

export interface RingHeader {
  slotCount: number;
  slotCapacity: number;
  writeIndex: number;
  audioClockFlicks: number;
  audioClockHostUs: number;
  heartbeatUs: number;
  frameDurationFlicks: number;
  enginePid: number;
  /** 0 starting, 1 playing, 2 paused, 3 stopped. */
  engineState: number;
  engineDropped: number;
  slots: (FrameHeader | null)[];
}

export interface FrameView {
  header: FrameHeader;
  buffer: ArrayBuffer;
  /** True if `buffer` aliases the shared memory (Node); false if the runtime copied (Electron). */
  zeroCopy: boolean;
}

export interface ReaderStats {
  frames: number;
  torn: number;
  nothingNew: number;
}

export declare class RingReader {
  private constructor();
  static open(name: string): RingReader;
  readonly closed: boolean;
  readonly slotCount: number;
  readonly capacity: number;
  readHeader(): RingHeader;
  latestHeader(): FrameHeader | null;
  readLatestInto(dst: Uint8Array, newerThan: number): FrameHeader | null;
  readSlotInto(slot: number, dst: Uint8Array): FrameHeader;
  latestView(): FrameView | null;
  isValid(h: FrameHeader): boolean;
  /** Transport (b): resolves with the write index once it exceeds `afterWriteIndex`, or after
   * `timeoutMs` (unchanged index). Polls every `pollUs` µs (default 250) off the JS thread. */
  waitForFrame(afterWriteIndex: number, timeoutMs: number, pollUs?: number): Promise<number>;
  stats(): ReaderStats;
  close(): void;
}

export declare class TestRingWriter {
  private constructor();
  static create(name: string, slots: number, width: number, height: number): TestRingWriter;
  publish(format: number, pts: number, fill: number): number;
  beginWrite(slot: number): void;
  endWrite(slot: number): void;
  corrupt(slot: number, offset: number): void;
  close(): void;
}

export declare function ringExists(name: string): boolean;
export declare function nowUs(): number;
export declare function checksum(data: Uint8Array, len: number): number;
