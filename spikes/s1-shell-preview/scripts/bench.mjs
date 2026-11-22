// SPDX-License-Identifier: GPL-3.0-or-later
// Run one benchmark (desktop/src/bench.ts) and forward its exit code.
// Usage: node scripts/bench.mjs [suite|ab] [quick|<seconds>] [--transport=utility|texture]
//                               [--draw=...] [--policy=...] [--size=WxH] [--warmup=<s>] ...
//   suite (default for transport (a)): the WP-0C.02 phases; <seconds> per playback phase.
//   ab    (default for --transport=utility|texture): one WP-0C.06 A/B run of <seconds>
//         (scripts/bench-ab.mjs runs the whole A/B matrix).
// Transport (a) defaults to the WP-0C.01 best mode: present loop in the preload,
// clock-scheduled frames. Transport (b) and the texture probe pick their own loop.
import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const electron = createRequire(import.meta.url)("electron"); // path to the binary
const args = process.argv.slice(2);
const benchMode = /^(suite|ab)$/.test(args[0] ?? "") ? args.shift() : "";
const length = /^(quick|\d+)$/.test(args[0] ?? "") ? args.shift() : "";
const transportA = !args.some((a) => a.startsWith("--transport=") && a !== "--transport=preload");
const defaults = (transportA ? ["--draw=preload", "--policy=clock"] : ["--policy=clock"]).filter(
  (d) => !args.some((a) => a.startsWith(d.split("=")[0])),
);
const child = spawn(
  electron,
  [
    root,
    length ? `--bench=${length}` : "--bench",
    ...(benchMode ? [`--bench-mode=${benchMode}`] : []),
    ...defaults,
    ...args,
  ],
  { stdio: "inherit" },
);
child.on("exit", (code) => process.exit(code ?? 1));
