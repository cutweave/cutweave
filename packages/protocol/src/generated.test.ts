// SPDX-License-Identifier: GPL-3.0-or-later
import { readdirSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const dir = new URL("./generated/", import.meta.url);
const names = ["commands.ts", "events.ts", "schema-hash.ts", "types.ts"];
const text = (name: string) => readFileSync(new URL(name, dir), "utf8");

describe("generated files", () => {
  it("are all present", () => {
    expect(readdirSync(dir).sort()).toEqual(names);
  });

  for (const name of names) {
    it(`${name}: header, LF, no \`any\`, no \`bigint\``, () => {
      const t = text(name);
      expect(t.startsWith("// @generated — do not edit; run cargo xtask gen\n")).toBe(true);
      expect(t).not.toContain("\r");
      // Doc comments may use the words; code must not use the types.
      const code = t
        .split("\n")
        .filter((l) => !/^\s*(\/\/|\/?\*)/.test(l))
        .join("\n");
      // A string literal `"any"` (an enum variant of the wire format) is not the `any` type.
      expect(code).not.toMatch(/(?<!")\bany\b(?!")/);
      expect(code).not.toMatch(/\bbigint\b/i);
    });
  }

  it("Time and Duration are number; IDs are branded strings", () => {
    const t = text("types.ts");
    expect(t).toContain("export type Time = number");
    expect(t).toContain("export type Duration = number");
    expect(t).toContain('export type MediaId = string & { readonly __brand: "MediaId" };');
    expect(t).toContain('export type ItemId = string & { readonly __brand: "ItemId" };');
  });

  it("the command map covers clip.split and the event map covers doc.patched", () => {
    expect(text("commands.ts")).toContain('"clip.split": { input: ClipSplit;');
    expect(text("events.ts")).toContain('"doc.patched": DocPatched;');
  });
});
