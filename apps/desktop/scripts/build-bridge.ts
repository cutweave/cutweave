// SPDX-License-Identifier: GPL-3.0-or-later
// Build the `cutweave-bridge` napi addon (WP-1H.03) and stage it where the main-window preload
// loads it: apps/desktop/native/cutweave_bridge.node. Usage (also `just bridge`):
//
//   node scripts/build-bridge.ts [--release] [--fixture]
//
// `--fixture` also builds the Rust test writer (`ring_fixture`, an example of the crate) that the
// Node harness and the Playwright e2e publish frames with. The addon is a plain cdylib: napi-rs
// v3 needs no post-processing, so the "napi build" is `cargo build` plus a rename to `.node`.
// Cross-compiling (win32-x64, darwin-arm64, linux-x64-gnu in CI) builds with `--target` and
// stages the same way; per-platform npm packaging arrives with the release pipeline.

import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const desktopDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(desktopDir, "..", "..");
const release = process.argv.includes("--release");
const fixture = process.argv.includes("--fixture");
const profile = release ? "release" : "debug";

const run = (args: string[]) =>
  execFileSync("cargo", args, { cwd: repoRoot, stdio: "inherit", encoding: "utf8" });

const cargoArgs = ["build", "-p", "cutweave-bridge", ...(release ? ["--release"] : [])];
run(cargoArgs); // the addon alone: no dev-dependency features leak into the shipped cdylib
if (fixture) run([...cargoArgs, "--example", "ring_fixture"]);

const metadata = JSON.parse(
  execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], {
    cwd: repoRoot,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  }),
) as { target_directory: string };
const targetDir = metadata.target_directory;

const library = {
  win32: "cutweave_bridge.dll",
  linux: "libcutweave_bridge.so",
  darwin: "libcutweave_bridge.dylib",
}[process.platform as string];
if (library === undefined) throw new Error(`unsupported platform ${process.platform}`);

const nativeDir = path.join(desktopDir, "native");
mkdirSync(nativeDir, { recursive: true });
const source = path.join(targetDir, profile, library);
const dest = path.join(nativeDir, "cutweave_bridge.node");
if (!existsSync(source)) throw new Error(`cargo did not produce ${source}`);
if (!existsSync(dest) || !readFileSync(source).equals(readFileSync(dest))) {
  try {
    copyFileSync(source, dest);
  } catch (err) {
    // Windows keeps a loaded .node locked (a running Electron or test worker).
    console.error(`build-bridge: cannot replace ${dest} (is the app still running?)`);
    throw err;
  }
}
console.log(`build-bridge: ${dest} (${profile})`);
if (fixture) {
  const exe = process.platform === "win32" ? "ring_fixture.exe" : "ring_fixture";
  console.log(`build-bridge: ${path.join(targetDir, profile, "examples", exe)}`);
}
