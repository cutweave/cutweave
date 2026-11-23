// SPDX-License-Identifier: GPL-3.0-or-later
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { PatchOp } from "./generated/types.ts";
import { applyPatch, type Json, jsonEqual, PatchError } from "./patch.ts";

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
