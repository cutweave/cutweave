// SPDX-License-Identifier: GPL-3.0-or-later
// Build the spike's Rust workspace (engine + napi addon) and stage the addon as
// bridge/s1_bridge.node. Usage: node scripts/build-native.mjs [--debug]
import { execFileSync } from "node:child_process";
import { copyFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const debug = process.argv.includes("--debug");
const profile = debug ? "debug" : "release";

execFileSync("cargo", ["build", "--workspace", ...(debug ? [] : ["--release"])], {
  cwd: root,
  stdio: "inherit",
});

const lib = {
  win32: "s1_bridge.dll",
  linux: "libs1_bridge.so",
  darwin: "libs1_bridge.dylib",
}[process.platform];
if (!lib) throw new Error(`unsupported platform ${process.platform}`);
const dest = join(root, "bridge", "s1_bridge.node");
try {
  copyFileSync(join(root, "target", profile, lib), dest);
} catch (e) {
  // On Windows a running Electron/Node keeps the .node locked.
  console.error(`could not stage ${dest} (is the spike still running?)`);
  throw e;
}
console.log(`staged ${dest} (${profile})`);
