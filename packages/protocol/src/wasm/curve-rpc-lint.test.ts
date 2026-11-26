// SPDX-License-Identifier: GPL-3.0-or-later
// Lint (WP-2C.06): UI code draws keyframe curves and evaluates animated values locally with
// `@cutweave/protocol/wasm` (`anim.evalRange`, `anim.eval`), never with an RPC per point.
// `keyframe.eval` is the engine query for agents and tools, so naming it in a string in
// `packages/*/src` fails here, outside generated code, tests, test support and the files listed
// below.

import { existsSync, readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { repoRoot } from "../test-support/math-wasm.ts";

const QUERY = ["keyframe", "eval"].join(".");

/** Files that may still call the query, and why. Each one is a debt with a work package. */
const ALLOWED: ReadonlyMap<string, string> = new Map([
  [
    "packages/protocol/src/mutating.ts",
    "the protocol's list of read-only methods; it calls nothing",
  ],
  [
    "packages/editor/src/inspector/useEvaluated.ts",
    "one throttled value per property at the playhead for the Inspector fields, not curve " +
      "drawing; moves to anim.eval when WP-2C.05 brings the module into the editor",
  ],
]);

/** The lines of `source` that name the query in a string literal (comments do not count). */
function curveRpcLines(source: string): number[] {
  const literal = new RegExp(`["'\`]${QUERY.replace(".", "\\.")}["'\`]`);
  const hits: number[] = [];
  let inBlock = false;
  source.split(/\r?\n/).forEach((line, i) => {
    let code = line;
    if (inBlock) {
      const end = code.indexOf("*/");
      if (end < 0) return;
      code = code.slice(end + 2);
      inBlock = false;
    }
    code = code.replace(/\/\*.*?\*\//g, "");
    const open = code.indexOf("/*");
    if (open >= 0) {
      code = code.slice(0, open);
      inBlock = true;
    }
    const comment = code.search(/(^|\s)\/\//);
    if (comment >= 0) code = code.slice(0, comment);
    if (literal.test(code)) hits.push(i + 1);
  });
  return hits;
}

/** Generated code and test support (fakes, fixtures) are not UI code. */
const SKIPPED_DIRS: ReadonlySet<string> = new Set([
  "generated",
  "node_modules",
  "testing",
  "test-support",
]);

function sources(dir: string, out: string[] = []): string[] {
  if (!existsSync(dir)) return out;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (!SKIPPED_DIRS.has(entry.name)) sources(p, out);
    } else if (/\.(ts|tsx)$/.test(entry.name) && !/\.test\.tsx?$/.test(entry.name)) {
      out.push(p);
    }
  }
  return out;
}

describe(`no ${QUERY} RPC for curve drawing in packages/*`, () => {
  it("finds the query in code and not in comments", () => {
    const q = QUERY;
    expect(curveRpcLines(`await rpc.call("${q}", { item });`)).toEqual([1]);
    expect(curveRpcLines(`useQuery('${q}', args)`)).toEqual([1]);
    expect(curveRpcLines(`// the engine's \`${q}\` ("${q}")`)).toEqual([]);
    expect(curveRpcLines(`/* "${q}" */ const a = 1;`)).toEqual([]);
    expect(curveRpcLines(`/**\n * "${q}"\n */\nrpc.call("${q}")`)).toEqual([4]);
    expect(curveRpcLines(`rpc.call("${q}uate")`)).toEqual([]);
    expect(curveRpcLines(`const url = "http://x"; call("${q}")`)).toEqual([1]);
  });

  it("no package source calls it outside the allowed files", () => {
    const packages = path.join(repoRoot, "packages");
    const offenders: string[] = [];
    const seen = new Set<string>();
    for (const pkg of readdirSync(packages, { withFileTypes: true })) {
      if (!pkg.isDirectory()) continue;
      for (const file of sources(path.join(packages, pkg.name, "src"), [])) {
        const rel = path.relative(repoRoot, file).replaceAll("\\", "/");
        const lines = curveRpcLines(readFileSync(file, "utf8"));
        if (lines.length === 0) continue;
        if (ALLOWED.has(rel)) seen.add(rel);
        else offenders.push(`${rel}:${lines.join(",")}`);
      }
    }
    expect(offenders, `use anim.eval / anim.evalRange from @cutweave/protocol/wasm`).toEqual([]);
    // A listed file that no longer calls it is struck off the list.
    expect([...ALLOWED.keys()].filter((f) => !seen.has(f))).toEqual([]);
  });
});
