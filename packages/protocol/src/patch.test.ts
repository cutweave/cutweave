// SPDX-License-Identifier: GPL-3.0-or-later
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { PatchOp } from "./generated/types.ts";
import { applyPatch, type Json, jsonEqual, PatchError, patchWork } from "./patch.ts";

interface Fixture {
  name: string;
  before: Json;
  after: Json;
  forward_patch: PatchOp[];
  inverse_patch: PatchOp[];
}

const fixtures = (
  JSON.parse(
    readFileSync(new URL("../../../schema/patch-fixtures.json", import.meta.url), "utf8"),
  ) as { fixtures: Fixture[] }
).fixtures;

describe("applyPatch against schema/patch-fixtures.json", () => {
  it("has the fixtures the engine emits", () => {
    expect(fixtures.length).toBeGreaterThanOrEqual(9);
  });

  for (const f of fixtures) {
    it(`${f.name}: forward (before to after)`, () => {
      const snapshot = structuredClone(f.before);
      expect(applyPatch(f.before, f.forward_patch)).toEqual(f.after);
      expect(f.before).toEqual(snapshot); // input untouched
    });

    it(`${f.name}: inverse (after to before)`, () => {
      expect(applyPatch(f.after, f.inverse_patch)).toEqual(f.before);
    });
  }
});

describe("applyPatch semantics", () => {
  it("shares unchanged subtrees", () => {
    const doc = { a: { x: 1 }, b: { y: 2 } };
    const next = applyPatch(doc, [{ op: "replace", path: "/a/x", old: 1, value: 3 }]);
    expect(next.b).toBe(doc.b);
    expect(next.a).not.toBe(doc.a);
  });

  it("rejects a replace whose old value no longer matches", () => {
    expect(() => applyPatch({ a: 1 }, [{ op: "replace", path: "/a", old: 2, value: 3 }])).toThrow(
      PatchError,
    );
  });

  it("add refuses an existing key and accepts array index 0..=len", () => {
    expect(() => applyPatch({ a: 1 }, [{ op: "add", path: "/a", value: 2 }])).toThrow(PatchError);
    expect(applyPatch({ l: [1, 2] }, [{ op: "add", path: "/l/2", value: 3 }])).toEqual({
      l: [1, 2, 3],
    });
    expect(applyPatch({ l: [1, 2] }, [{ op: "add", path: "/l/0", value: 0 }])).toEqual({
      l: [0, 1, 2],
    });
    expect(() => applyPatch({ l: [1] }, [{ op: "add", path: "/l/3", value: 0 }])).toThrow(
      PatchError,
    );
  });

  it("move removes first, then adds", () => {
    const doc = { a: { k: 1 }, b: {} };
    expect(applyPatch(doc, [{ op: "move", from: "/a/k", path: "/b/k" }])).toEqual({
      a: {},
      b: { k: 1 },
    });
  });

  it("unescapes ~0 and ~1 in pointers", () => {
    expect(applyPatch({ "a/b": { "~": 1 } }, [{ op: "remove", path: "/a~1b/~0", old: 1 }])).toEqual(
      { "a/b": {} },
    );
  });

  it("reports the failing op index", () => {
    try {
      applyPatch({}, [
        { op: "add", path: "/a", value: 1 },
        { op: "remove", path: "/zzz", old: 1 },
      ]);
      expect.unreachable();
    } catch (error) {
      expect((error as PatchError).opIndex).toBe(1);
    }
  });

  it("jsonEqual compares objects regardless of key order", () => {
    expect(jsonEqual({ a: 1, b: [1, 2] }, { b: [1, 2], a: 1 })).toBe(true);
    expect(jsonEqual({ a: 1 }, { a: 2 })).toBe(false);
  });
});

// Issue #231: a ripple reaches the editor as one op per clip it shifted. Each op used to copy the
// path from the root again, the track's whole map of clips with it: 2,000 ops, 2,000 copies of
// 2,000 entries. Counted, not timed, so the test holds on a busy machine.
describe("applyPatch on a patch of many ops", () => {
  const clips = (n: number): Record<string, { start: number; name: string }> =>
    Object.fromEntries(
      Array.from({ length: n }, (_, i) => [`itm_${i}`, { start: i * 10, name: `c${i}` }]),
    );
  const project = (n: number) => ({
    meta: { name: "p" },
    sequences: { seq_1: { tracks: { trk_a: { items: clips(n) }, trk_b: { items: clips(3) } } } },
  });
  const ripple = (n: number, by: number): PatchOp[] =>
    Array.from({ length: n }, (_, i) => ({
      op: "replace" as const,
      path: `/sequences/seq_1/tracks/trk_a/items/itm_${i}/start`,
      old: i * 10,
      value: i * 10 + by,
    }));

  it("copies each container on the way once, however many ops pass through it", () => {
    const counts = [20, 2000].map((n) => {
      const doc = project(n);
      const frozen = structuredClone(doc);
      const before = patchWork.copies;
      const next = applyPatch(doc as unknown as Json, ripple(n, 5)) as unknown as typeof doc;
      const copies = patchWork.copies - before;
      // The document given is untouched, the other track is shared, every clip moved.
      expect(doc).toEqual(frozen);
      expect(next.sequences.seq_1.tracks.trk_b).toBe(doc.sequences.seq_1.tracks.trk_b);
      expect(next.meta).toBe(doc.meta);
      const items = next.sequences.seq_1.tracks.trk_a.items;
      expect(Object.keys(items)).toEqual(Object.keys(doc.sequences.seq_1.tracks.trk_a.items));
      expect(Object.values(items).every((c, i) => c.start === i * 10 + 5)).toBe(true);
      return copies - n;
    });
    // The root, `sequences`, the sequence, `tracks`, the track and `items`: six, plus one a clip.
    expect(counts).toEqual([6, 6]);
  });

  it("gives what applying the ops one patch at a time gives", () => {
    const doc = project(12) as unknown as Json;
    const patch: PatchOp[] = [
      ...ripple(12, 7),
      {
        op: "remove",
        path: "/sequences/seq_1/tracks/trk_a/items/itm_3",
        old: { start: 37, name: "c3" },
      },
      {
        op: "add",
        path: "/sequences/seq_1/tracks/trk_a/items/itm_x",
        value: { start: 1, name: "x" },
      },
      {
        op: "replace",
        path: "/sequences/seq_1/tracks/trk_a/items/itm_x/name",
        old: "x",
        value: "y",
      },
      {
        op: "move",
        from: "/sequences/seq_1/tracks/trk_a/items/itm_4",
        path: "/sequences/seq_1/tracks/trk_b/items/itm_4",
      },
      { op: "replace", path: "/sequences/seq_1/tracks/trk_b/items/itm_4/start", old: 47, value: 0 },
      { op: "add", path: "/order", value: ["a", "c"] },
      { op: "add", path: "/order/1", value: "b" },
      { op: "remove", path: "/order/0", old: "a" },
      { op: "replace", path: "/order/1", old: "c", value: "d" },
    ] as unknown as PatchOp[];
    const stepwise = patch.reduce<Json>((d, op) => applyPatch(d, [op]), doc);
    const at_once = applyPatch(doc, patch);
    expect(at_once).toEqual(stepwise);
    // Member order is part of what the editor sees (lists are built from it).
    expect(JSON.stringify(at_once)).toBe(JSON.stringify(stepwise));
    // A value an op brought in is never changed in place by a later op.
    expect(patch[13]).toEqual({
      op: "add",
      path: "/sequences/seq_1/tracks/trk_a/items/itm_x",
      value: { start: 1, name: "x" },
    });
    expect(patch[17]).toEqual({ op: "add", path: "/order", value: ["a", "c"] });
  });

  it("leaves the document as it was when a later op does not apply", () => {
    const doc = project(4);
    const frozen = structuredClone(doc);
    const patch = [
      ...ripple(4, 5),
      { op: "replace", path: "/sequences/seq_1/tracks/trk_a/items/itm_0/start", old: 99, value: 1 },
    ] as unknown as PatchOp[];
    expect(() => applyPatch(doc as unknown as Json, patch)).toThrow(PatchError);
    expect(doc).toEqual(frozen);
  });
});
