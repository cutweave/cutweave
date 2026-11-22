// SPDX-License-Identifier: GPL-3.0-or-later
// Run the WP-0C.02 benchmark / self-test (desktop/src/bench.ts) and forward its exit code.
// Usage: node scripts/bench.mjs [quick|<seconds per playback phase>] [--draw=main] [--policy=latest]
// Defaults to the WP-0C.01 best mode: present loop in the preload, clock-scheduled frames.
import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const electron = createRequire(import.meta.url)("electron"); // path to the binary
const args = process.argv.slice(2);
const mode = /^(quick|\d+)$/.test(args[0] ?? "") ? args.shift() : "";
const defaults = ["--draw=preload", "--policy=clock"].filter(
  (d) => !args.some((a) => a.startsWith(d.split("=")[0])),
);
const child = spawn(
  electron,
  [root, mode ? `--bench=${mode}` : "--bench", ...defaults, ...args],
  { stdio: "inherit" },
);
child.on("exit", (code) => process.exit(code ?? 1));
