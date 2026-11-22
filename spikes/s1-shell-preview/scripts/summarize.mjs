// SPDX-License-Identifier: GPL-3.0-or-later
// Print a Markdown table from results/selftest-*.json (informal numbers; WP-0C.04 owns the
// formal aggregation via `cargo xtask bench-report`).
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const dir = join(dirname(fileURLToPath(import.meta.url)), "..", "results");
const files = readdirSync(dir).filter((f) => f.startsWith("selftest-") && f.endsWith(".json"));
const f2 = (x) => (typeof x === "number" && Number.isFinite(x) ? x.toFixed(1) : "-");
const pp = (l) => (l ? `${f2(l.p50)} / ${f2(l.p95)}` : "-");

const rows = files.sort().map((f) => {
  const r = JSON.parse(readFileSync(join(dir, f), "utf8"));
  const o = r.options ?? {};
  const run = r.run ?? {};
  const L = run.latency ?? {};
  const cpu = r.cpuPercentOfOneCore ?? {};
  return [
    `${r.transport === "utility" ? `(b) utility/${o.drawB}/` : ""}${o.format}/${r.transport === "utility" ? "" : `${o.draw}/`}${o.policy}${o.policy === "clock" ? `@${o.latency}ms` : ""}/${o.present}${o.chromiumDgpu ? "/dgpu" : ""}${o.verify === "0" ? "/noverify" : ""}`,
    run.seconds,
    run.presentFps,
    run.seqGapsPerMin,
    `${run.checksumFailures ?? "-"}/${run.rowStampFailures ?? "-"}`,
    pp(L.renderToPresentMs),
    pp(L.publishToPresentMs),
    pp(L.addonCopyMs),
    pp(L.cloneMs),
    pp(L.uploadDrawMs),
    `${f2(cpu.engine)} / ${f2(cpu.Tab)} / ${f2(cpu.GPU)} / ${f2(cpu.Browser)}`,
    `${f2(r.crash?.detectMs)} / ${f2(r.crash?.pipeCloseMs)}`,
    r.pass ? "PASS" : "FAIL",
  ];
});

const head = [
  "format/draw/policy/present",
  "s",
  "present fps",
  "seq gaps/min",
  "cksum/stamp fails",
  "render->present p50/p95 ms",
  "publish->present p50/p95",
  "addon copy p50/p95",
  "bridge clone p50/p95",
  "upload+draw p50/p95",
  "CPU % engine/renderer/GPU/main",
  "kill->exit / pipe-close ms",
  "checks",
];
console.log(`| ${head.join(" | ")} |`);
console.log(`|${head.map(() => "---").join("|")}|`);
for (const r of rows) console.log(`| ${r.join(" | ")} |`);
