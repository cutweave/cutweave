// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import { mulberry32 } from "./fixtures.ts";
import { IntervalIndex } from "./interval-index.ts";

function brute(starts: number[], ends: number[], a: number, b: number): number[] {
  const out: number[] = [];
  starts.forEach((s, i) => {
    if (s <= b && (ends[i] as number) >= a) out.push(i);
  });
  return out.sort((x, y) => (starts[x] as number) - (starts[y] as number) || x - y);
}

describe("IntervalIndex", () => {
  it("is empty-safe", () => {
    const idx = new IntervalIndex([], [], []);
    expect(idx.size).toBe(0);
    expect(idx.query(0, 100, [])).toEqual([]);
  });

  it("matches a brute-force scan on random overlapping intervals", () => {
    const rnd = mulberry32(7);
    const n = 400;
    const starts: number[] = [];
    const ends: number[] = [];
    for (let i = 0; i < n; i++) {
      const s = Math.floor(rnd() * 10_000);
      starts.push(s);
      // Mix of short clips and a few very long ones (the prefix-max scan's worst case).
      ends.push(s + (rnd() < 0.05 ? Math.floor(rnd() * 8000) : Math.floor(rnd() * 120)));
    }
    const idx = new IntervalIndex(
      Array.from({ length: n }, (_, i) => i),
      starts,
      ends,
    );
    const out: number[] = [];
    for (let q = 0; q < 300; q++) {
      const a = Math.floor(rnd() * 10_000);
      const b = a + Math.floor(rnd() * 500);
      expect(idx.query(a, b, out)).toEqual(brute(starts, ends, a, b));
    }
  });

  it("treats intervals as closed and supports point queries", () => {
    const idx = new IntervalIndex([0, 1], [10, 30], [20, 40]);
    expect(idx.stab(20, [])).toEqual([0]); // the end is inclusive
    expect(idx.stab(25, [])).toEqual([]);
    expect(idx.stab(30, [])).toEqual([1]); // so is the start
    expect(idx.stab(9, [])).toEqual([]);
    expect(idx.stab(10, [])).toEqual([0]);
    expect(idx.query(41, 50, [])).toEqual([]);
  });

  it("indexes a subset of ids and returns ascending start order", () => {
    const starts = [50, 10, 30, 20];
    const ends = [60, 15, 35, 25];
    const idx = new IntervalIndex([0, 2, 3], starts, ends); // id 1 is not in this track
    expect(idx.query(0, 100, [])).toEqual([3, 2, 0]);
  });

  it("reuses the output buffer without allocating a new array", () => {
    const idx = new IntervalIndex([0], [0], [5]);
    const out = [99, 98];
    expect(idx.query(0, 1, out)).toBe(out);
    expect(out).toEqual([0]);
  });

  it("answers a window query over 250 items touching only a handful", () => {
    const n = 250;
    const starts = Array.from({ length: n }, (_, i) => i * 10);
    const ends = starts.map((s) => s + 8);
    const idx = new IntervalIndex(
      starts.map((_, i) => i),
      starts,
      ends,
    );
    expect(idx.query(1000, 1030, [])).toEqual([100, 101, 102, 103]);
  });
});
