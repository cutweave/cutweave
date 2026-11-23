// SPDX-License-Identifier: GPL-3.0-or-later
import type { RequestId } from "./generated/types.ts";

/** Fills `bytes` with random values. `crypto` exists in browsers, Electron and Node 24. */
export type RandomBytes = (bytes: Uint8Array<ArrayBuffer>) => void;

const defaultRandom: RandomBytes = (bytes) => {
  globalThis.crypto.getRandomValues(bytes);
};

/**
 * A UUIDv7 (RFC 9562): 48 bits of Unix milliseconds, then random bits. The engine's
 * `request_id` is a client-minted idempotency key (arch §5.1, ADR-0015); time-ordered keys keep
 * the engine's dedup window and the journal easy to read. The wall clock is fine here: this is
 * UI code, not render or command code.
 */
export function uuidv7(nowMs: number = Date.now(), random: RandomBytes = defaultRandom): RequestId {
  const bytes = new Uint8Array(16);
  random(bytes);
  // Millisecond timestamp, big-endian, in the first 6 bytes (division: `>>` would truncate).
  let ms = Math.max(0, Math.floor(nowMs));
  for (let i = 5; i >= 0; i--) {
    bytes[i] = ms % 256;
    ms = Math.floor(ms / 256);
  }
  bytes[6] = ((bytes[6] ?? 0) & 0x0f) | 0x70; // version 7
  bytes[8] = ((bytes[8] ?? 0) & 0x3f) | 0x80; // variant 10
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}` as RequestId;
}
