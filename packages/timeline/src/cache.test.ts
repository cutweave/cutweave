// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from "vitest";
import {
  bucketPxPerSec,
  FILMSTRIP_BYTES,
  LoadingCache,
  MAX_TILE_INDEX,
  MediaCaches,
  zoomBucket,
} from "./cache.ts";

const tick = (): Promise<void> => new Promise((r) => setTimeout(r, 0));

function make(maxBytes = 100, concurrency = 6) {
  const onLoaded = vi.fn();
  const dispose = vi.fn();
  const cache = new LoadingCache<{ n: number }>({
    maxBytes,
    sizeOf: () => 40,
    dispose,
    onLoaded,
    concurrency,
  });
  return { cache, onLoaded, dispose };
}

// Keys are numbers (the renderer packs them; see MediaCaches).
const A = 1;
const B = 2;
const C = 3;
const D = 4;

describe("LoadingCache", () => {
  it("loads on a miss, then serves hits without reloading", async () => {
    const { cache, onLoaded } = make();
    const load = vi.fn(async () => ({ n: 1 }));
    expect(cache.getOrLoad(A, load)).toBeUndefined();
    await tick();
    expect(onLoaded).toHaveBeenCalledTimes(1);
    expect(cache.getOrLoad(A, load)).toEqual({ n: 1 });
    expect(load).toHaveBeenCalledTimes(1);
  });

  it("de-duplicates concurrent requests for one key and says when a load is needed", async () => {
    const { cache } = make();
    const load = vi.fn(async () => ({ n: 1 }));
    expect(cache.needsLoad(A)).toBe(true);
    cache.getOrLoad(A, load);
    expect(cache.needsLoad(A)).toBe(false); // queued
    cache.getOrLoad(A, load);
    cache.getOrLoad(A, load);
    await tick();
    expect(load).toHaveBeenCalledTimes(1);
    expect(cache.needsLoad(A)).toBe(false); // cached
  });

  it("evicts least recently used entries past the byte budget and disposes them", async () => {
    const { cache, dispose } = make(100); // room for two 40-byte entries
    for (const k of [A, B]) {
      cache.getOrLoad(k, async () => ({ n: 0 }));
      await tick();
    }
    cache.get(A); // refresh a: b is now the oldest
    cache.getOrLoad(C, async () => ({ n: 0 }));
    await tick();
    expect(cache.has(A)).toBe(true);
    expect(cache.has(B)).toBe(false);
    expect(cache.has(C)).toBe(true);
    expect(dispose).toHaveBeenCalledTimes(1);
    expect(cache.byteSize).toBe(80);
  });

  it("peeks without refreshing recency or starting a load", async () => {
    const { cache } = make(100);
    const load = vi.fn(async () => ({ n: 0 }));
    expect(cache.peek(A)).toBeUndefined();
    expect(cache.pending).toBe(0);
    for (const k of [A, B]) {
      cache.getOrLoad(k, load);
      await tick();
    }
    expect(cache.peek(A)).toEqual({ n: 0 }); // a stays the oldest
    cache.getOrLoad(C, load);
    await tick();
    expect(cache.has(A)).toBe(false);
    expect(cache.has(B)).toBe(true);
  });

  it("does not retry failed or empty loads until cleared", async () => {
    const { cache } = make();
    const bad = vi.fn(async () => {
      throw new Error("404");
    });
    const none = vi.fn(async () => null);
    cache.getOrLoad(A, bad);
    cache.getOrLoad(B, none);
    await tick();
    cache.getOrLoad(A, bad);
    cache.getOrLoad(B, none);
    await tick();
    expect(bad).toHaveBeenCalledTimes(1);
    expect(none).toHaveBeenCalledTimes(1);
    cache.clear();
    cache.getOrLoad(A, bad);
    await tick();
    expect(bad).toHaveBeenCalledTimes(2);
  });

  it("bounds concurrency and drops queued loads that were not wanted in the last frame", async () => {
    const { cache } = make(10_000, 2);
    const resolvers: Array<() => void> = [];
    const started: number[] = [];
    const slow = (key: number) => () => {
      started.push(key);
      return new Promise<{ n: number }>((resolve) => resolvers.push(() => resolve({ n: 0 })));
    };
    cache.beginFrame();
    for (const k of [A, B, C, D]) cache.getOrLoad(k, slow(k));
    await tick();
    expect(started).toHaveLength(2); // concurrency 2
    expect(started).toEqual([D, C]); // newest first
    cache.beginFrame();
    cache.beginFrame(); // two frames pass: a and b are stale
    for (const r of resolvers) r();
    await tick();
    await tick();
    expect(started).toEqual([D, C]);
    expect(cache.pending).toBe(0);
  });

  it("aborts a running load the last full frame did not want; partial frames keep it", async () => {
    const { cache } = make();
    const signals = new Map<string, AbortSignal>();
    const finishers = new Map<string, (v: { n: number }) => void>();
    const slow = (name: string) => (s: AbortSignal) => {
      signals.set(name, s);
      return new Promise<{ n: number }>((r) => finishers.set(name, r));
    };
    cache.beginFrame();
    cache.getOrLoad(A, slow("a"));
    cache.getOrLoad(B, slow("b"));
    await tick();
    // Partial redraws (the playhead) ask for nothing: nothing is aborted.
    cache.beginFrame(false);
    cache.beginFrame(false);
    cache.beginFrame(false);
    expect(signals.get("a")?.aborted).toBe(false);
    // A full frame still wants `a` (it is on screen) but not `b` (scrolled away)...
    cache.beginFrame();
    cache.get(A);
    expect(signals.get("b")?.aborted).toBe(false);
    // ...so the next full frame aborts `b` only.
    cache.beginFrame();
    expect(signals.get("a")?.aborted).toBe(false);
    expect(signals.get("b")?.aborted).toBe(true);
    finishers.get("b")?.({ n: 2 });
    finishers.get("a")?.({ n: 1 });
    await tick();
    expect(cache.has(A)).toBe(true);
    // An aborted load is not a failure: asking again loads it again.
    expect(cache.has(B)).toBe(false);
    cache.getOrLoad(B, slow("b2"));
    await tick();
    expect(signals.has("b2")).toBe(true);
  });

  it("aborts in-flight loads and discards their results on clear", async () => {
    const { cache, dispose } = make();
    let signal: AbortSignal | undefined;
    let finish: (v: { n: number }) => void = () => {};
    cache.getOrLoad(A, (s) => {
      signal = s;
      return new Promise((r) => {
        finish = r;
      });
    });
    await tick();
    cache.clear();
    expect(signal?.aborted).toBe(true);
    finish({ n: 1 });
    await tick();
    expect(cache.has(A)).toBe(false);
    expect(dispose).toHaveBeenCalledTimes(1);
  });
});

describe("MediaCaches", () => {
  /** A stand-in bitmap of `w` x `h` pixels (4 bytes each). */
  const bitmap = (w: number, h: number) =>
    ({ width: w, height: h, close: vi.fn() }) as unknown as ImageBitmap;

  it("bounds the filmstrip bitmaps to 256 MB, least recently used out first", async () => {
    // 1024 x 1024 x 4 = 4 MiB per tile: 100 tiles would be 400 MiB.
    const tile = vi.fn(async () => bitmap(1024, 1024));
    const caches = new MediaCaches({ loaders: { filmstripTile: tile }, onLoaded: () => {} });
    expect(caches.filmstrip.maxBytes).toBe(FILMSTRIP_BYTES);
    expect(FILMSTRIP_BYTES).toBe(256 * 1024 * 1024);
    const media = caches.mediaSlot("med_1");
    const size = caches.sizeSlot(96, 54);
    const req = { mediaId: "med_1", sourceTime: 0, tileDuration: 1, width: 96, height: 54 };
    for (let j = 0; j < 100; j++) {
      caches.beginFrame();
      const key = caches.tileKey(media, size, 10, j);
      if (caches.filmstrip.needsLoad(key)) caches.requestTile(key, req);
      // The first tile stays in use every frame, so it is never the least recently used.
      caches.filmstrip.get(caches.tileKey(media, size, 10, 0));
      await tick();
      expect(caches.filmstrip.byteSize).toBeLessThanOrEqual(FILMSTRIP_BYTES);
    }
    expect(tile).toHaveBeenCalledTimes(100);
    expect(caches.filmstrip.size).toBeLessThan(64);
    expect(caches.filmstrip.has(caches.tileKey(media, size, 10, 0))).toBe(true);
    expect(caches.filmstrip.has(caches.tileKey(media, size, 10, 1))).toBe(false);
  });

  it("packs distinct, exact integer keys", () => {
    const caches = new MediaCaches({ loaders: {}, onLoaded: () => {} });
    const keys = new Set<number>();
    const medias = [caches.mediaSlot("a"), caches.mediaSlot("b"), caches.mediaSlot("a")];
    expect(medias).toEqual([0, 1, 0]);
    const sizes = [caches.sizeSlot(96, 54), caches.sizeSlot(128, 72)];
    for (const m of [0, 1, 2 ** 15 - 1]) {
      for (const s of [...sizes, 31]) {
        for (const b of [-13, 0, 29]) {
          for (const j of [0, 1, MAX_TILE_INDEX - 1]) {
            const k = caches.tileKey(m, s, b, j);
            expect(Number.isSafeInteger(k)).toBe(true);
            keys.add(k);
          }
        }
      }
    }
    expect(keys.size).toBe(3 * 3 * 3 * 3);
    expect(caches.peaksKey(0, 13)).not.toBe(caches.peaksKey(0, 14));
    expect(caches.peaksKey(0, 13)).not.toBe(caches.peaksKey(1, 13));
  });
});

describe("zoom buckets", () => {
  it("are half-octave steps that map back to a nearby zoom", () => {
    expect(zoomBucket(1)).toBe(0);
    expect(zoomBucket(2)).toBe(2);
    expect(zoomBucket(100)).toBe(13);
    for (const pps of [3, 17, 100, 640]) {
      const ratio = bucketPxPerSec(zoomBucket(pps)) / pps;
      expect(ratio).toBeGreaterThan(0.8);
      expect(ratio).toBeLessThan(1.25);
    }
  });
});
