// SPDX-License-Identifier: GPL-3.0-or-later
// WP-0C.06 A/B benchmark: transport (a) vs (b), informal, this machine.
//
//   node scripts/bench-ab.mjs [seconds=60] [--sizes=1920x1080,1280x720] [--format=rgba8|nv12]
//                             [--only=a,b-main,b-worker,texture] [--latency=20] [--warmup=5]
//                             [--quiet-below=150] [--max-wait=600] [--retry-foreign=250] [--retries=2]
//                             [--priority=above|normal]
//   node scripts/bench-ab.mjs --table           # print the table from results/bench-*.json
//
// Each configuration is a fresh Electron launch with `--bench=<seconds> --bench-mode=ab`
// (desktop/src/bench.ts; the WP-0C.02 suite files in results/ are skipped by --table):
// warm-up, reset counters, run, record fps, skipped frames/min (seq gaps), engine GPU submit ->
// renderer present submit p50/p95, per-process CPU. Runs are interleaved a, b, a, b, ... per size
// so slow drift (thermals, background load) hits both transports alike. Before each run it waits
// (up to --max-wait s) until the whole machine is below --quiet-below % of one core, and every
// result records "foreign" CPU (machine minus our processes): other builds/agents share the box.
import { spawn } from "node:child_process";
import { mkdirSync, readdirSync, readFileSync, renameSync } from "node:fs";
import { createRequire } from "node:module";
import { cpus } from "node:os";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const resultsDir = join(root, "results");
const args = process.argv.slice(2);
const opt = (name, dflt) => {
  const hit = args.find((a) => a.startsWith(`--${name}=`));
  return hit ? hit.slice(name.length + 3) : dflt;
};

const CONFIGS = {
  a: { label: "(a) preload addon, draw in preload", flags: ["--transport=preload", "--draw=preload"] },
  "b-main": { label: "(b) utility + MessagePort, draw on renderer main thread", flags: ["--transport=utility", "--draw=main"] },
  "b-worker": { label: "(b) utility + MessagePort, draw in renderer worker", flags: ["--transport=utility", "--draw=worker"] },
  texture: { label: "sharedTexture GPU handoff probe (OSR source)", flags: ["--transport=texture"] },
};

/** Whole-machine CPU busy, in % of one core (100 = one core), over `ms`. */
async function machineLoad(ms) {
  const snap = () => {
    let busy = 0;
    let total = 0;
    for (const c of cpus()) {
      const t = c.times;
      busy += t.user + t.nice + t.sys + t.irq;
      total += t.user + t.nice + t.sys + t.irq + t.idle;
    }
    return { busy, total };
  };
  const a = snap();
  await new Promise((r) => setTimeout(r, ms));
  const b = snap();
  return ((b.busy - a.busy) / Math.max(1, b.total - a.total)) * cpus().length * 100;
}

/** Other agents/builds share this machine: wait (bounded) until it is quiet before a run. */
async function waitForQuiet(belowPct, maxWaitS) {
  const until = Date.now() + maxWaitS * 1000;
  for (;;) {
    const load = await machineLoad(3000);
    if (load < belowPct) return { load, waited: false };
    if (Date.now() > until) return { load, waited: true };
    console.log(`  machine busy (${load.toFixed(0)}% of one core); waiting...`);
    await new Promise((r) => setTimeout(r, 7000));
  }
}

function runOne(flags) {
  const electron = createRequire(import.meta.url)("electron");
  return new Promise((resolve) => {
    let file = null;
    const child = spawn(electron, [root, ...flags], { stdio: ["ignore", "pipe", "pipe"] });
    const onLine = (d) => {
      for (const line of d.toString().split(/\r?\n/)) {
        const m = /^\[bench\] (.+\.json)$/.exec(line.trim());
        if (m) file = m[1];
        if (line.startsWith("[bench]") || /^\[(main|utility|texture)\].*(error|fail)/i.test(line)) {
          console.log(`  ${line}`);
        }
      }
    };
    child.stdout.on("data", onLine);
    child.stderr.on("data", onLine);
    child.on("exit", (code) => resolve({ code, file }));
  });
}

/** A/B result (bench.ts `ab` mode). Files from before `mode` existed: `summary` marks A/B. */
const isAb = (r) => r.kind === "s1-bench" && (r.mode ?? (r.summary ? "ab" : "suite")) === "ab";

const f1 = (x) => (typeof x === "number" && Number.isFinite(x) ? x.toFixed(1) : "-");
const f2 = (x) => (typeof x === "number" && Number.isFinite(x) ? x.toFixed(2) : "-");

function table(files) {
  const rows = files
    .map((f) => JSON.parse(readFileSync(f, "utf8")))
    .filter(isAb)
    .sort((x, y) =>
      `${x.summary.size}${x.summary.format}${x.summary.transport}${x.summary.drawIn}${x.options?.latency}`.localeCompare(
        `${y.summary.size}${y.summary.format}${y.summary.transport}${y.summary.drawIn}${y.options?.latency}`,
      ),
    );
  const head = [
    "size / format",
    "transport / loop in",
    "frame choice",
    "s",
    "present fps",
    "skipped /min",
    "engine drops",
    "submit -> draw p50 / p95 ms",
    "CPU % renderer",
    "CPU % utility",
    "CPU % GPU / main / engine",
    "foreign load",
    "torn",
  ];
  const out = [`| ${head.join(" | ")} |`, `|${head.map(() => "---").join("|")}|`];
  for (const r of rows) {
    const s = r.summary;
    const tr = { utility: "(b)", texture: "sharedTexture probe" }[s.transport] ?? "(a)";
    const loop = s.drawIn.split(" ")[0];
    const util =
      s.cpu.utility !== null && s.cpu.utility !== undefined
        ? f1(s.cpu.utility)
        : s.cpu.producer !== undefined
          ? `n/a (OSR source ${f1(s.cpu.producer)})`
          : "n/a";
    out.push(
      `| ${s.size} ${s.format} | ${tr} ${loop} | ${s.policy === "clock" ? `clock ${r.options?.latency ?? "?"} ms` : s.policy} | ${f1(s.seconds)} | ${f2(s.presentFps)} | ${f1(s.skippedPerMin)} | ${s.engineDropped ?? "-"} | ${f1(s.renderToPresentMs?.p50)} / ${f1(s.renderToPresentMs?.p95)} | ${f1(s.cpu.renderer)} | ${util} | ${f1(s.cpu.gpu)} / ${f1(s.cpu.main)} / ${f1(s.cpu.engine)} | ${f1(s.cpu.foreign)} | ${s.tornFrames} |`,
    );
  }
  return out.join("\n");
}

function transportStages(files) {
  const out = [];
  for (const f of files) {
    const r = JSON.parse(readFileSync(f, "utf8"));
    if (!isAb(r) || r.summary.transport !== "utility") continue;
    const u = r.extra?.utility;
    const rx = r.snapshot?.receiver;
    if (!u || !rx) continue;
    const pp = (l) => (l ? `${f2(l.p50)} / ${f2(l.p95)}` : "-");
    out.push(
      `| ${r.summary.size} ${r.summary.format} ${r.summary.drawIn.split(" ")[0]} clock ${r.options?.latency} ms | ${pp(u.publishToWakeMs)} | ${pp(u.copyMs)} | ${pp(u.postMs)} | ${pp(rx.deserializeMs)} | ${pp(rx.publishToReceiveMs)} | ${u.creditDrops} | ${rx.queueDrops} |`,
    );
  }
  if (out.length === 0) return "";
  return [
    "| (b) size / format / loop | publish -> utility wake | shm -> heap copy | postMessage (serialize + BigBuffer) | renderer `ev.data` deserialize | publish -> renderer receipt | credit drops | rx queue drops |",
    "|---|---|---|---|---|---|---|---|",
    ...out.sort(),
  ].join("\n");
}

const listResults = () =>
  readdirSync(resultsDir)
    .filter((f) => f.startsWith("bench-") && f.endsWith(".json"))
    .map((f) => join(resultsDir, f));

if (args.includes("--table")) {
  const files = listResults();
  console.log(table(files));
  console.log();
  console.log(transportStages(files));
  process.exit(0);
}

const seconds = /^\d+$/.test(args[0] ?? "") ? args[0] : "60";
const sizes = opt("sizes", "1920x1080,1280x720").split(",");
const only = opt("only", "a,b-main,b-worker").split(",");
const shared = [
  `--bench=${seconds}`,
  "--bench-mode=ab",
  `--warmup=${opt("warmup", "5")}`,
  "--policy=clock",
  `--latency=${opt("latency", "20")}`,
  `--format=${opt("format", "rgba8")}`,
  // Above-normal priority for every measured process, both transports alike (shared dev box);
  // --priority=normal measures without it.
  ...(opt("priority", "above") === "above" ? ["--bench-priority=above"] : []),
];
// A run whose foreign load (machine minus our processes) exceeded --retry-foreign (% of one
// core) is moved to results/exploratory/contaminated/ and repeated, up to --retries times.
const retryForeign = Number(opt("retry-foreign", "250"));
const retries = Number(opt("retries", "2"));
const contaminated = join(resultsDir, "exploratory", "contaminated");
const produced = [];
for (const size of sizes) {
  for (const key of only) {
    const c = CONFIGS[key];
    if (!c) throw new Error(`unknown config ${key}`);
    console.log(`${size}: ${c.label}`);
    for (let attempt = 0; attempt <= retries; attempt++) {
      const q = await waitForQuiet(Number(opt("quiet-below", "150")), Number(opt("max-wait", "600")));
      console.log(`  machine load before run: ${q.load.toFixed(0)}% of one core${q.waited ? " (gave up waiting)" : ""}`);
      const { code, file } = await runOne([...c.flags, ...shared, `--size=${size}`]);
      if (code !== 0 || !file) {
        console.log(`  FAILED (exit ${code})`);
        break;
      }
      const foreign = JSON.parse(readFileSync(file, "utf8")).summary?.cpu?.foreign ?? 0;
      if (foreign > retryForeign && attempt < retries) {
        mkdirSync(contaminated, { recursive: true });
        renameSync(file, join(contaminated, basename(file)));
        console.log(`  foreign load ${foreign.toFixed(0)}% during the run: retrying`);
        continue;
      }
      produced.push(file);
      break;
    }
  }
}
console.log();
console.log(table(produced));
console.log();
console.log(transportStages(produced));
