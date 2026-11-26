// SPDX-License-Identifier: GPL-3.0-or-later
// Makes sure the math module is built before something bundles it (WP-2C.06, ADR-0033): the
// editor's Vite config calls `ensureMathModule()` for `just dev`, the e2e build and the packaged
// build, so a fresh checkout needs no separate `just wasm`. A module older than its Rust sources
// is rebuilt the same way (`node packages/protocol/wasm/build.ts --no-typings`).

import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const wasmDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(wasmDir, "..", "..", "..");

/** The staged module (not committed). */
export const STAGED_MODULE = path.join(wasmDir, "cutweave_math.wasm");

/** Everything the module is compiled from: the C ABI crate, the model it wraps and the lockfile. */
const SOURCES = ["crates/cutweave-wasm", "crates/cutweave-model", "Cargo.lock", "Cargo.toml"];

/** The newest modification time (ms) under `entry`, ignoring build output; 0 when it is missing. */
function newest(entry: string): number {
  if (!existsSync(entry)) return 0;
  const stat = statSync(entry);
  if (!stat.isDirectory()) return stat.mtimeMs;
  let latest = 0; // a directory's own time changes with its entries, not its content
  for (const name of readdirSync(entry)) {
    if (name === "target" || name === "node_modules") continue;
    latest = Math.max(latest, newest(path.join(entry, name)));
  }
  return latest;
}

/** True when the module is missing or older than a source it is built from. */
export function isMathModuleStale(root: string = repoRoot, staged: string = STAGED_MODULE): boolean {
  if (!existsSync(staged)) return true;
  const built = statSync(staged).mtimeMs;
  return SOURCES.some((source) => newest(path.join(root, source)) > built);
}

/** Builds the module when it is missing or stale; a no-op otherwise. Returns whether it built. */
export function ensureMathModule(): boolean {
  if (!isMathModuleStale()) return false;
  console.log("wasm: the math module is missing or stale; building it (just wasm)");
  execFileSync(process.execPath, [path.join(wasmDir, "build.ts"), "--no-typings"], {
    cwd: repoRoot,
    stdio: "inherit",
  });
  return true;
}
