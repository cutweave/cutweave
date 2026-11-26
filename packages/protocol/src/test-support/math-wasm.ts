// SPDX-License-Identifier: GPL-3.0-or-later
// The staged math module for tests (WP-2C.06): builds it first (`wasm/build.ts --no-typings`)
// when it is missing or older than the Rust it is built from, so `pnpm test` never checks a stale
// binary. The gate builds it beforehand (`just wasm --check`), so there this only reads it.

import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const protocolDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
export const repoRoot = path.resolve(protocolDir, "..", "..");
export const MODULE_PATH = path.join(protocolDir, "wasm", "cutweave_math.wasm");
const BUILD_SCRIPT = path.join(protocolDir, "wasm", "build.ts");

/** What the module is built from. */
const SOURCES = [
  "crates/cutweave-wasm/src",
  "crates/cutweave-wasm/Cargo.toml",
  "crates/cutweave-model/src",
  "crates/cutweave-model/Cargo.toml",
  "Cargo.toml",
  "Cargo.lock",
];

function newest(p: string): number {
  if (!existsSync(p)) return 0;
  const st = statSync(p);
  if (!st.isDirectory()) return st.mtimeMs;
  return readdirSync(p).reduce((m, name) => Math.max(m, newest(path.join(p, name))), st.mtimeMs);
}

/** The module's bytes, built first when missing or stale. */
export function mathModuleBytes(): Uint8Array {
  const sources = Math.max(...SOURCES.map((s) => newest(path.join(repoRoot, s))));
  if (!existsSync(MODULE_PATH) || statSync(MODULE_PATH).mtimeMs < sources) {
    const built = spawnSync(process.execPath, [BUILD_SCRIPT, "--no-typings"], {
      cwd: repoRoot,
      stdio: "inherit",
    });
    if (built.status !== 0) throw new Error("building the math module failed (see output above)");
  }
  return new Uint8Array(readFileSync(MODULE_PATH));
}
