// SPDX-License-Identifier: GPL-3.0-or-later
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { needsRequestId, READ_ONLY_METHODS } from "./mutating.ts";
import { uuidv7 } from "./request-id.ts";

interface Entry {
  name: string;
  annotations: { readOnlyHint: boolean };
}

const read = (file: string) =>
  readFileSync(new URL(`../../../schema/${file}`, import.meta.url), "utf8");

/** Commands and queries (`schema/queries.json`): every registry entry. */
const catalog: Entry[] = [
  ...(JSON.parse(read("commands.json")) as { commands: Entry[] }).commands,
  ...(JSON.parse(read("queries.json")) as { queries: Entry[] }).queries,
];

describe("needsRequestId", () => {
  it("agrees with schema/commands.json: every non-read-only entry (view.* aside) needs one", () => {
    for (const { name, annotations } of catalog) {
      const expected = !annotations.readOnlyHint && !name.startsWith("view.");
      expect(needsRequestId(name), name).toBe(expected);
    }
  });

  it("READ_ONLY_METHODS has no entry the catalog does not know (besides reserved methods)", () => {
    const known = new Set(catalog.map((c) => c.name));
    for (const method of READ_ONLY_METHODS) {
      expect(
        known.has(method) || method.startsWith("rpc.") || method === "project.snapshot",
        method,
      ).toBe(true);
    }
  });

  it("the reserved methods carry none", () => {
    for (const method of ["rpc.hello", "rpc.subscribe", "rpc.unsubscribe", "project.snapshot"]) {
      expect(needsRequestId(method)).toBe(false);
    }
  });
});

describe("uuidv7", () => {
  it("has version 7, variant 10 and the timestamp in the first 48 bits", () => {
    const id = uuidv7(0x0190f5c2a1b2);
    expect(id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    expect(id.replaceAll("-", "").slice(0, 12)).toBe("0190f5c2a1b2");
  });

  it("is unique and sorts by time", () => {
    const a = uuidv7(1_000_000);
    const b = uuidv7(1_000_001);
    expect(a < b).toBe(true);
    expect(new Set(Array.from({ length: 200 }, () => uuidv7(5))).size).toBe(200);
  });
});
