// SPDX-License-Identifier: GPL-3.0-or-later
import { mkdirSync, mkdtempSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { isMathModuleStale } from "./ensure.ts";

describe("isMathModuleStale", () => {
  const dirs: string[] = [];
  afterEach(() => {
    for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
  });

  function fixture(): { root: string; staged: string; source: string } {
    const root = mkdtempSync(path.join(tmpdir(), "cw-ensure-"));
    dirs.push(root);
    mkdirSync(path.join(root, "crates", "cutweave-model", "src"), { recursive: true });
    const source = path.join(root, "crates", "cutweave-model", "src", "lib.rs");
    writeFileSync(source, "// model");
    return { root, staged: path.join(root, "cutweave_math.wasm"), source };
  }

  const at = (file: string, seconds: number) => utimesSync(file, seconds, seconds);

  it("is stale when the module is missing", () => {
    const { root, staged } = fixture();
    expect(isMathModuleStale(root, staged)).toBe(true);
  });

  it("is fresh when the module is newer than every source", () => {
    const { root, staged, source } = fixture();
    writeFileSync(staged, "wasm");
    at(source, 1_000);
    at(staged, 2_000);
    expect(isMathModuleStale(root, staged)).toBe(false);
  });

  it("is stale when a source is newer than the module", () => {
    const { root, staged, source } = fixture();
    writeFileSync(staged, "wasm");
    at(staged, 1_000);
    at(source, 2_000);
    expect(isMathModuleStale(root, staged)).toBe(true);
  });
});
