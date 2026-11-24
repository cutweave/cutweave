// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it, vi } from "vitest";
import { bucketPxPerSec, LoadingCache, zoomBucket } from "./cache.ts";

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

describe("LoadingCache", () => {
  it("loads on a miss, then serves hits without reloading", async () => {
    const { cache, onLoaded } = make();
    const load = vi.fn(async () => ({ n: 1 }));
    expect(cache.get("a", load)).toBeUndefined();
    await tick();
    expect(onLoaded).toHaveBeenCalledTimes(1);
    expect(cache.get("a", load)).toEqual({ n: 1 });
    expect(load).toHaveBeenCalledTimes(1);
  });

  it("de-duplicates concurrent requests for one key", async () => {
    const { cache } = make();
    const load = vi.fn(async () => ({ n: 1 }));
    cache.get("a", load);
    cache.get("a", load);
    cache.get("a", load);
    await tick();
    expect(load).toHaveBeenCalledTimes(1);
  });

  it("evicts least recently used entries past the byte budget and disposes them", async () => {
    const { cache, dispose } = make(100); // room for two 40-byte entries
    for (const k of ["a", "b"]) {
      cache.get(k, async () => ({ n: 0 }));
      await tick();
    }
    cache.get("a", undefined); // refresh a: b is now the oldest
    cache.get("c", async () => ({ n: 0 }));
    await tick();
    expect(cache.has("a")).toBe(true);
    expect(cache.has("b")).toBe(false);
    expect(cache.has("c")).toBe(true);
    expect(dispose).toHaveBeenCalledTimes(1);
    expect(cache.byteSize).toBe(80);
  });

  it("does not retry failed or empty loads until cleared", async () => {
    const { cache } = make();
    const bad = vi.fn(async () => {
      throw new Error("404");
    });
    const none = vi.fn(async () => null);
    cache.get("bad", bad);
    cache.get("none", none);
    await tick();
    cache.get("bad", bad);
    cache.get("none", none);
    await tick();
    expect(bad).toHaveBeenCalledTimes(1);
    expect(none).toHaveBeenCalledTimes(1);
    cache.clear();
    cache.get("bad", bad);
    await tick();
    expect(bad).toHaveBeenCalledTimes(2);
  });

  it("bounds concurrency and drops queued loads that were not wanted in the last frame", async () => {
    const { cache } = make(10_000, 2);
    const resolvers: Array<() => void> = [];
    const started: string[] = [];
    const slow = (key: string) => () => {
      started.push(key);
      return new Promise<{ n: number }>((resolve) => resolvers.push(() => resolve({ n: 0 })));
    };
    cache.beginFrame();
    for (const k of ["a", "b", "c", "d"]) cache.get(k, slow(k));
    await tick();
    expect(started).toHaveLength(2); // concurrency 2
    expect(started).toEqual(["d", "c"]); // newest first
    cache.beginFrame();
    cache.beginFrame(); // two frames pass: a and b are stale
    for (const r of resolvers) r();
    await tick();
    await tick();
    expect(started).toEqual(["d", "c"]);
    expect(cache.pending).toBe(0);
  });

  it("aborts a running load the last full frame did not want; partial frames keep it", async () => {
    const { cache } = make();
    const signals = new Map<string, AbortSignal>();
    const finishers = new Map<string, (v: { n: number }) => void>();
    const slow = (key: string) => (s: AbortSignal) => {
      signals.set(key, s);
      return new Promise<{ n: number }>((r) => finishers.set(key, r));
    };
    cache.beginFrame();
    cache.get("a", slow("a"));
    cache.get("b", slow("b"));
    await tick();
    // Partial redraws (the playhead) ask for nothing: nothing is aborted.
    cache.beginFrame(false);
    cache.beginFrame(false);
    cache.beginFrame(false);
    expect(signals.get("a")?.aborted).toBe(false);
    // A full frame still wants `a` (it is on screen) but not `b` (scrolled away)...
    cache.beginFrame();
    cache.get("a", slow("a"));
    expect(signals.get("b")?.aborted).toBe(false);
    // ...so the next full frame aborts `b` only.
    cache.beginFrame();
    expect(signals.get("a")?.aborted).toBe(false);
    expect(signals.get("b")?.aborted).toBe(true);
    finishers.get("b")?.({ n: 2 });
    finishers.get("a")?.({ n: 1 });
    await tick();
    expect(cache.has("a")).toBe(true);
    // An aborted load is not a failure: asking again loads it again.
    expect(cache.has("b")).toBe(false);
    cache.get("b", slow("b2"));
    await tick();
    expect(signals.has("b2")).toBe(true);
  });

  it("aborts in-flight loads and discards their results on clear", async () => {
    const { cache, dispose } = make();
    let signal: AbortSignal | undefined;
    let finish: (v: { n: number }) => void = () => {};
    cache.get("a", (s) => {
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
    expect(cache.has("a")).toBe(false);
    expect(dispose).toHaveBeenCalledTimes(1);
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
