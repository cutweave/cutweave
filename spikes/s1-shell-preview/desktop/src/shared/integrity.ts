// SPDX-License-Identifier: GPL-3.0-or-later
// TS mirror of ring/src/checksum.rs (RING.md "Integrity"). Used by the renderer to prove that
// every presented frame is untorn, and by bridge/test to cross-check against the Rust version.
// Erasable-only TypeScript so Node can import it directly (type stripping).

export const CHECKSUM_WORD_STRIDE = 61;
export const ROW_STAMP_LEN = 8;

const FNV_OFFSET = 0x811c9dc5;
const FNV_PRIME = 0x01000193;

/** Word-wise FNV-1a over every 61st little-endian u32 of the first `len` bytes, then `len`. */
export function sampledFnv1a(bytes: Uint8Array, len: number = bytes.byteLength): number {
  const dv = new DataView(bytes.buffer, bytes.byteOffset, len);
  const words = Math.floor(len / 4);
  let h = FNV_OFFSET;
  for (let i = 0; i < words; i += CHECKSUM_WORD_STRIDE) {
    h = Math.imul(h ^ dv.getUint32(i * 4, true), FNV_PRIME);
  }
  return Math.imul(h ^ len, FNV_PRIME) >>> 0;
}

/** Number of plane-0 rows whose 8-byte stamp is not `seq` (u64 LE). */
export function badRowStamps(
  bytes: Uint8Array,
  stride: number,
  rows: number,
  seq: number,
): number {
  const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const lo = seq % 0x1_0000_0000;
  const hi = Math.floor(seq / 0x1_0000_0000);
  let bad = 0;
  for (let r = 0; r < rows; r++) {
    const o = r * stride;
    if (
      o + ROW_STAMP_LEN > bytes.byteLength ||
      dv.getUint32(o, true) !== lo ||
      dv.getUint32(o + 4, true) !== hi
    ) {
      bad++;
    }
  }
  return bad;
}
