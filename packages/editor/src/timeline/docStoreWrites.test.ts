// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2A.10 acceptance, "no UI mutation of docStore outside patch events" (arch §5.1, §16): a lint
// over the editor's sources. The document projection changes only from the engine: `DocSync`
// applies `doc.patched` events and snapshots through `docSink` (engine/runtime.ts), which calls
// the store's own `applyEvent` and `resync` (state/docStore.ts). Any other write fails here: a
// `setState` on the doc store, a direct `applyEvent` or `resync`, or a second `docSink`. The
// runtime half (every store change during a drag comes with a new revision) is in
// `timeline.integration.test.ts`.
import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

const SRC = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");

/** The two places allowed to write: the store itself, and the engine runtime's `DocSync`. */
const ALLOWED: Readonly<Record<string, string>> = {
  "state/docStore.ts": "the store: applyEvent and resync are its only writers",
  "engine/runtime.ts": "DocSync applies doc.patched and project.snapshot through docSink",
};

/** Ways to write the document store. */
const WRITES: readonly { readonly name: string; readonly pattern: RegExp }[] = [
  {
    name: "setState on the doc store",
    pattern: /\b(?:docStore|doc|stores\.doc|deps\.doc|this\.#deps\.doc)\s*\.\s*setState\s*\(/,
  },
  { name: "applyEvent outside DocSync", pattern: /\.\s*applyEvent\s*\(/ },
  { name: "resync outside DocSync", pattern: /\.\s*resync\s*\(\s*[a-zA-Z{]/ },
  { name: "a second docSink", pattern: /\bdocSink\s*\(/ },
];

function sources(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const full = path.join(dir, name);
    if (statSync(full).isDirectory()) return name === "testing" ? [] : sources(full);
    return /\.tsx?$/.test(name) && !/\.(test|stories)\.tsx?$/.test(name) ? [full] : [];
  });
}

/** Comments may name the methods; only code counts. */
function code(text: string): string {
  return text.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/.*$/gm, "$1");
}

/** `file:line: what` for every write outside the allowed files. */
function violations(files: readonly string[]): string[] {
  const out: string[] = [];
  for (const file of files) {
    const rel = path.relative(SRC, file).split(path.sep).join("/");
    if (ALLOWED[rel] !== undefined) continue;
    const lines = code(readFileSync(file, "utf8")).split("\n");
    lines.forEach((line, i) => {
      for (const w of WRITES) if (w.pattern.test(line)) out.push(`${rel}:${i + 1}: ${w.name}`);
    });
  }
  return out;
}

describe("the UI never writes the document store", () => {
  it("finds no write outside DocSync and the store", () => {
    expect(violations(sources(SRC))).toEqual([]);
  });

  it("would catch each kind of write (the rule is not vacuous)", () => {
    const planted = [
      "docStore.setState({ project: null });",
      "doc.getState().applyEvent(event);",
      "stores.doc.getState().resync(snapshot);",
      "const sink = docSink(store);",
    ];
    for (const line of planted) {
      expect(
        WRITES.some((w) => w.pattern.test(line)),
        line,
      ).toBe(true);
    }
    // The allowed files really do write (so the allowlist is still needed).
    for (const rel of Object.keys(ALLOWED)) {
      const text = code(readFileSync(path.join(SRC, rel), "utf8"));
      expect(
        WRITES.some((w) => w.pattern.test(text)),
        rel,
      ).toBe(true);
    }
  });
});
