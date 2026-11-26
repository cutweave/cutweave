// SPDX-License-Identifier: GPL-3.0-or-later
// Builds the UI's shared math module (WP-2C.06, arch §16) and stages it for
// `@cutweave/protocol/wasm`. Usage (also `just wasm`):
//
//   node packages/protocol/wasm/build.ts [--check | --no-typings]
//
// 1. `cargo build -p cutweave-wasm --target wasm32-unknown-unknown --profile wasm` (plain cargo:
//    the crate exports a C ABI, so no wasm-bindgen, wasm-pack or wasm-opt is needed);
// 2. copies the module to packages/protocol/wasm/cutweave_math.wasm (not committed);
// 3. regenerates packages/protocol/src/wasm/generated/exports.ts from the module's own exports
//    (`--check` fails when the committed file differs instead; `--no-typings` skips the step);
// 4. checks the gzipped size against the 150 KB budget.

import { execFileSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  readFileSync,
  renameSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";
import { readAbi, renderExportsModule } from "../src/wasm/abi.ts";

/** Gzipped size budget of the module (WP-2C.06). */
export const GZIP_BUDGET_BYTES = 150_000;

const wasmDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(wasmDir, "..", "..", "..");
const staged = path.join(wasmDir, "cutweave_math.wasm");
const typings = path.resolve(wasmDir, "..", "src", "wasm", "generated", "exports.ts");

function main(): number {
  const check = process.argv.includes("--check");
  const skipTypings = process.argv.includes("--no-typings");
  execFileSync(
    "cargo",
    ["build", "-p", "cutweave-wasm", "--target", "wasm32-unknown-unknown", "--profile", "wasm"],
    { cwd: repoRoot, stdio: "inherit" },
  );
  const metadata = JSON.parse(
    execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps", "--offline"], {
      cwd: repoRoot,
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
    }),
  ) as { target_directory: string };
  const built = path.join(
    metadata.target_directory,
    "wasm32-unknown-unknown",
    "wasm",
    "cutweave_wasm.wasm",
  );
  if (!existsSync(built)) throw new Error(`cargo did not produce ${built}`);
  const bytes = readFileSync(built);
  if (!existsSync(staged) || !readFileSync(staged).equals(bytes)) {
    // Through a temporary file, so a reader never sees half a module.
    const tmp = `${staged}.${process.pid}.tmp`;
    copyFileSync(built, tmp);
    renameSync(tmp, staged);
  } else {
    // Up to date with its sources, even when they changed without changing the module.
    const now = new Date();
    utimesSync(staged, now, now);
  }

  let ok = true;
  if (!skipTypings) {
    const version = moduleVersion(bytes);
    const text = renderExportsModule(readAbi(bytes), version);
    const current = existsSync(typings) ? readFileSync(typings, "utf8").replace(/\r\n/g, "\n") : "";
    if (current !== text) {
      if (check) {
        console.error(
          `wasm: ${path.relative(repoRoot, typings)} is out of date with the module's exports; run \`just wasm\``,
        );
        ok = false;
      } else {
        writeFileSync(typings, text);
        console.log(`wasm: regenerated ${path.relative(repoRoot, typings)}`);
      }
    }
  }

  const gzipped = gzipSync(bytes, { level: 9 }).length;
  const kb = (n: number) => `${(n / 1000).toFixed(1)} KB`;
  console.log(
    `wasm: ${path.relative(repoRoot, staged)}: ${kb(bytes.length)}, ${kb(gzipped)} gzipped (budget ${kb(GZIP_BUDGET_BYTES)})`,
  );
  if (gzipped > GZIP_BUDGET_BYTES) {
    console.error("wasm: the module is over its gzipped size budget");
    ok = false;
  }
  return ok ? 0 : 1;
}

/** The version the module reports (`cw_version`), read from a throwaway instance. */
function moduleVersion(bytes: Uint8Array): string {
  const instance = new WebAssembly.Instance(new WebAssembly.Module(bytes), {});
  const x = instance.exports as {
    memory: WebAssembly.Memory;
    cw_version(): number;
    cw_out_ptr(): number;
  };
  const len = x.cw_version();
  return new TextDecoder().decode(new Uint8Array(x.memory.buffer, x.cw_out_ptr() >>> 0, len));
}

process.exitCode = main();
