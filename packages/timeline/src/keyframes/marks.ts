// SPDX-License-Identifier: GPL-3.0-or-later
// The key marks of one track's keyframe lanes, collected in one pass and filled per shape and
// state (WP-2C.05). One long-lived buffer in the renderer's state: no allocation per frame.
import { KEY_GLYPHS } from "./lanes.ts";

/** The bar a collapsed run of keys draws as. */
export const CLUSTER = KEY_GLYPHS.length;
/** Buckets: (4 glyphs + cluster) x (plain, selected). */
export const BUCKETS = (CLUSTER + 1) * 2;

/** The marks of one track, collected in one pass and filled per bucket. */
export class KeyMarks {
  x = new Float32Array(256);
  y = new Float32Array(256);
  bucket = new Uint8Array(256);
  length = 0;
  readonly counts = new Int32Array(BUCKETS);

  clear(): void {
    this.length = 0;
    this.counts.fill(0);
  }

  push(x: number, y: number, bucket: number): void {
    if (this.length === this.x.length) {
      const n = this.length * 2;
      const nx = new Float32Array(n);
      const ny = new Float32Array(n);
      const nb = new Uint8Array(n);
      nx.set(this.x);
      ny.set(this.y);
      nb.set(this.bucket);
      this.x = nx;
      this.y = ny;
      this.bucket = nb;
    }
    this.x[this.length] = x;
    this.y[this.length] = y;
    this.bucket[this.length] = bucket;
    this.counts[bucket] = (this.counts[bucket] ?? 0) + 1;
    this.length++;
  }
}
