// SPDX-License-Identifier: GPL-3.0-or-later
import { existsSync, readFileSync, statSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const here = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(resolve(here, "fonts.css"), "utf8");
const blocks = [...css.matchAll(/@font-face\s*\{([^}]*)\}/g)].map((m) => m[1] ?? "");

const FAMILIES = {
  "Instrument Sans": "instrumentsans",
  "Bricolage Grotesque": "bricolagegrotesque",
  "JetBrains Mono": "jetbrainsmono",
} as const;

describe("fonts manifest", () => {
  it("declares latin and latin-ext faces for each family", () => {
    for (const family of Object.keys(FAMILIES)) {
      const faces = blocks.filter((b) => b.includes(`font-family: "${family}"`));
      expect(faces, family).toHaveLength(2);
    }
  });

  it("uses font-display: swap, woff2 format and a unicode-range on every face", () => {
    expect(blocks.length).toBe(6);
    for (const b of blocks) {
      expect(b).toContain("font-display: swap");
      expect(b).toMatch(/format\("woff2"\)/);
      expect(b).toContain("unicode-range:");
    }
  });

  it("declares variable weight ranges (not a single value)", () => {
    for (const b of blocks) {
      expect(b).toMatch(/font-weight:\s*\d+\s+\d+;/);
    }
  });

  it("references only files that exist and are non-empty woff2", () => {
    const urls = [...css.matchAll(/url\("([^"]+)"\)/g)].map((m) => m[1] ?? "");
    expect(urls).toHaveLength(6);
    for (const u of urls) {
      const path = resolve(here, u);
      expect(existsSync(path), u).toBe(true);
      expect(statSync(path).size, u).toBeGreaterThan(1000);
      expect(readFileSync(path).subarray(0, 4).toString("ascii"), u).toBe("wOF2");
    }
  });

  it("ships the OFL text beside each family", () => {
    for (const dir of Object.values(FAMILIES)) {
      const p = resolve(here, dir, "OFL.txt");
      expect(existsSync(p), dir).toBe(true);
      expect(readFileSync(p, "utf8")).toContain("SIL OPEN FONT LICENSE Version 1.1");
    }
  });

  it("matches the family names used by the typography tokens", () => {
    const tokens = readFileSync(resolve(here, "../tokens/tokens.css"), "utf8");
    for (const family of Object.keys(FAMILIES)) {
      expect(tokens).toContain(`"${family}"`);
    }
  });
});
