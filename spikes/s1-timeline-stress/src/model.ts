// SPDX-License-Identifier: GPL-3.0-or-later
// Synthetic timeline data (structure-of-arrays) and the per-track interval index.
// Spike code: time is plain seconds (f64), NOT flicks; see README "Findings".

export type Kind = "media" | "audio" | "graphics" | "captions" | "effects";

export const TRACK_KINDS: readonly Kind[] = [
  ...Array<Kind>(8).fill("media"),
  ...Array<Kind>(4).fill("audio"),
  ...Array<Kind>(3).fill("graphics"),
  ...Array<Kind>(3).fill("captions"),
  ...Array<Kind>(2).fill("effects"),
];

export const CLIPS_PER_TRACK = 250;
export const KEYFRAMES_PER_CLIP = 10;
export const THUMB_COUNT = 200;

/** Deterministic PRNG (AGENTS.md: seeded, never unseeded RNG). */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/**
 * Interval index for one track: clip ids sorted by start plus a prefix maximum of
 * end times. A range query binary-searches the last start <= b, then walks backwards
 * until the prefix max end drops below a. O(log n + k) for non-overlapping tracks
 * (the common case) and still correct when clips overlap.
 */
export class IntervalIndex {
  readonly ids: Int32Array;
  private readonly starts: Float64Array;
  private readonly ends: Float64Array;
  private readonly prefixMaxEnd: Float64Array;

  constructor(ids: number[], allStart: Float64Array, allEnd: Float64Array) {
    const sorted = ids.slice().sort((x, y) => (allStart[x] ?? 0) - (allStart[y] ?? 0));
    this.ids = Int32Array.from(sorted);
    this.starts = Float64Array.from(sorted, (i) => allStart[i] ?? 0);
    this.ends = Float64Array.from(sorted, (i) => allEnd[i] ?? 0);
    this.prefixMaxEnd = new Float64Array(sorted.length);
    let m = Number.NEGATIVE_INFINITY;
    for (let i = 0; i < sorted.length; i++) {
      m = Math.max(m, this.ends[i] ?? 0);
      this.prefixMaxEnd[i] = m;
    }
  }

  /** Appends the ids of clips overlapping [a, b] to `out` (cleared first). */
  query(a: number, b: number, out: number[]): number[] {
    out.length = 0;
    let lo = 0;
    let hi = this.starts.length;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if ((this.starts[mid] ?? 0) <= b) lo = mid + 1;
      else hi = mid;
    }
    for (let i = lo - 1; i >= 0; i--) {
      if ((this.prefixMaxEnd[i] ?? 0) < a) break;
      if ((this.ends[i] ?? 0) >= a) out.push(this.ids[i] ?? 0);
    }
    return out;
  }
}

export interface Timeline {
  readonly trackCount: number;
  readonly clipCount: number;
  readonly keyframeCount: number;
  readonly duration: number;
  readonly start: Float64Array;
  readonly end: Float64Array;
  readonly track: Uint8Array;
  /** Offset and count into `kfTime` (sorted ascending per clip, absolute seconds). */
  readonly kfOff: Uint32Array;
  readonly kfCount: Uint16Array;
  readonly kfTime: Float64Array;
  readonly index: readonly IntervalIndex[];
}

export function generateTimeline(seed = 1): Timeline {
  const rnd = mulberry32(seed);
  const trackCount = TRACK_KINDS.length;
  const clipCount = trackCount * CLIPS_PER_TRACK;
  const keyframeCount = clipCount * KEYFRAMES_PER_CLIP;
  const start = new Float64Array(clipCount);
  const end = new Float64Array(clipCount);
  const track = new Uint8Array(clipCount);
  const kfOff = new Uint32Array(clipCount);
  const kfCount = new Uint16Array(clipCount);
  const kfTime = new Float64Array(keyframeCount);
  const perTrack: number[][] = Array.from({ length: trackCount }, () => []);

  let id = 0;
  let kfAt = 0;
  let duration = 0;
  for (let t = 0; t < trackCount; t++) {
    let cursor = rnd() * 5;
    for (let c = 0; c < CLIPS_PER_TRACK; c++) {
      const len = 2 + rnd() * 22;
      start[id] = cursor;
      end[id] = cursor + len;
      track[id] = t;
      kfOff[id] = kfAt;
      kfCount[id] = KEYFRAMES_PER_CLIP;
      const times: number[] = [];
      for (let k = 0; k < KEYFRAMES_PER_CLIP; k++) times.push(cursor + rnd() * len);
      times.sort((x, y) => x - y);
      for (const tm of times) kfTime[kfAt++] = tm;
      perTrack[t]?.push(id);
      cursor += len + (rnd() < 0.3 ? 0 : rnd() * 3);
      id++;
    }
    duration = Math.max(duration, cursor);
  }
  const index = perTrack.map((ids) => new IntervalIndex(ids, start, end));
  return {
    trackCount,
    clipCount,
    keyframeCount,
    duration,
    start,
    end,
    track,
    kfOff,
    kfCount,
    kfTime,
    index,
  };
}
