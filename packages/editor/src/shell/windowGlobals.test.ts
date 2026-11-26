// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2H.15: panel code may be shown in a detached panel's own window, so the bare window globals
// are banned there by a lint rule (biome.json, `noRestrictedGlobals`), outside an allowlist. This
// lints a fixture with the repository's own Biome configuration, placed as panel code, as a test
// and as an allowlisted file (in a temporary copy of the layout: Biome reports no diagnostics for
// standard input).
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterAll, beforeAll, describe, expect, it } from "vitest";

const ROOT = fileURLToPath(new URL("../../../../", import.meta.url));
const BIOME = join(ROOT, "node_modules", "@biomejs", "biome", "bin", "biome");

/** Panel code that reaches for the main window instead of its own. */
const FIXTURE = `export function measure(el: HTMLElement): number {
  const observer = new ResizeObserver(() => {});
  observer.observe(el);
  requestAnimationFrame(() => {});
  cancelAnimationFrame(0);
  window.addEventListener("keydown", () => {});
  return document.body.clientWidth * devicePixelRatio;
}
`;

let dir = "";

beforeAll(() => {
  dir = mkdtempSync(join(tmpdir(), "cw-window-globals-"));
  const config = JSON.parse(readFileSync(join(ROOT, "biome.json"), "utf8")) as Record<
    string,
    unknown
  >;
  // The copy is no git checkout: no ignore file to read.
  config.vcs = { enabled: false };
  writeFileSync(join(dir, "biome.json"), JSON.stringify(config));
});

afterAll(() => {
  if (dir !== "") rmSync(dir, { recursive: true, force: true });
});

/** The rule's findings for `FIXTURE` placed at `path` (repository-relative). */
function findings(path: string): string[] {
  const file = join(dir, path);
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, FIXTURE);
  const run = spawnSync(
    process.execPath,
    [BIOME, "lint", `--config-path=${dir}`, "--max-diagnostics=50", path],
    { cwd: dir, encoding: "utf8" },
  );
  const out = `${run.stdout}\n${run.stderr}`;
  return [...out.matchAll(/Do not use the global variable (\w+)/g)].map((m) => m[1] as string);
}

describe("the window-globals lint rule", { timeout: 60_000 }, () => {
  it.skipIf(!existsSync(BIOME))("flags each bare window global in panel code", () => {
    expect(findings("packages/editor/src/panels/Fixture.tsx").sort()).toEqual([
      "ResizeObserver",
      "cancelAnimationFrame",
      "devicePixelRatio",
      "document",
      "requestAnimationFrame",
      "window",
    ]);
    expect(findings("packages/ui/src/components/Fixture.tsx")).toHaveLength(6);
    expect(findings("packages/timeline/src/fixture.ts")).toHaveLength(6);
  });

  it.skipIf(!existsSync(BIOME))("leaves tests, the allowlist and other code alone", () => {
    expect(findings("packages/editor/src/panels/Fixture.test.ts")).toEqual([]);
    expect(findings("packages/editor/src/shell/windows.ts")).toEqual([]);
    expect(findings("packages/ui/src/components/MenuBar.tsx")).toEqual([]);
    expect(findings("apps/desktop/src/preload/fixture.ts")).toEqual([]);
  });
});
