// SPDX-License-Identifier: GPL-3.0-or-later
// Run the Electron self-test (WP-0C.01 acceptance checks) and forward its exit code.
// Usage: node scripts/selftest.mjs [seconds] [--format=nv12] [--draw=preload] [--policy=next]
//                                  [--present=2d] [--verify=0] [--gpu=low|high|software]
import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const electron = createRequire(import.meta.url)("electron"); // path to the binary
const args = process.argv.slice(2);
const seconds = /^\d+$/.test(args[0] ?? "") ? args.shift() : "60";
const child = spawn(electron, [root, `--selftest=${seconds}`, ...args], { stdio: "inherit" });
child.on("exit", (code) => process.exit(code ?? 1));
