// SPDX-License-Identifier: GPL-3.0-or-later
// `pnpm --filter @cutweave/desktop perf:ui`: the UI benchmarks (WP-2J.02, bench/README.md).
//
//   node scripts/perf-ui.ts [--no-build] [--no-media] [--no-renderer] [--no-lock]
//                           [--slow RATE] [--out FILE]
//
// 1. Builds what the run measures: the engine and `cutweave-bench` in release, and the app.
//    With the FFmpeg bundle in the environment (`. scripts/ffmpeg/env.sh` first) the engine is
//    built with FFmpeg and the projects get their source files, so the timeline draws real
//    thumbnails and waveforms; without it the engine probes by file name (stub media).
// 2. Generates the benchmark projects (P-400, P-5K, P-short) into testdata/bench-projects/.
// 3. Runs `e2e/perf/ui.perf.ts` on the hidden desktop (scripts/hidden-desktop.py) under the
//    machine's GPU lock (scripts/gpu-lock.py; `--no-lock` on a machine that runs nothing else:
//    the lock also lowers the run's priority).
// 4. Runs the timeline renderer's own benchmark on P-5K (`packages/timeline/bench`, headless
//    Chromium, no app) and adds its numbers, so a slow frame can be told apart: the renderer,
//    or the app around it.
// 5. Writes bench-results/{machine}/{date}.ui.json, prints every metric with its budget and
//    PASS or FAIL, and exits 1 when one is over budget. `cargo xtask bench-check <file>
//    [--baseline <file>]` judges the same file again (the CI gate, WP-2J.04).
//
// `--slow RATE` slows the editor's main thread by RATE (Chromium's CPU throttling) while the
// timeline is panned and zoomed, to show that the budgets catch a slow build: such a run must
// fail the frame budgets, and its result goes to {date}.ui-slowed.json so it is never taken for
// a measurement.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  type BenchResult,
  defaultResultPath,
  failed,
  loadBudgets,
  readResult,
  table,
  writeResult,
} from "../e2e/perf/result.ts";

const desktopDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const root = path.resolve(desktopDir, "..", "..");
const exe = (name: string) => (process.platform === "win32" ? `${name}.exe` : name);

const args = process.argv.slice(2);
const KNOWN = ["--no-build", "--no-media", "--no-renderer", "--no-lock", "--slow", "--out"];
const value = (flag: string): string | undefined => {
  const at = args.indexOf(flag);
  return at < 0 ? undefined : args[at + 1];
};
for (const [i, arg] of args.entries()) {
  const isValue = i > 0 && (args[i - 1] === "--slow" || args[i - 1] === "--out");
  if (!isValue && !KNOWN.includes(arg)) {
    console.error(`perf-ui: unknown option ${arg}\n  options: ${KNOWN.join(" ")}`);
    process.exit(2);
  }
}
const slow = Number(value("--slow") ?? 1);
if (!Number.isFinite(slow) || slow < 1) {
  console.error("perf-ui: --slow wants a factor of 1 or more");
  process.exit(2);
}

/** Runs a command to its end and returns its exit code; its output is this script's. */
function run(
  command: readonly string[],
  options: { cwd?: string; env?: Record<string, string>; minutes?: number } = {},
): number {
  console.info(`\n> ${command.join(" ")}`);
  const quoted = command.map((part) => (/[\s"]/.test(part) ? `"${part}"` : part));
  // On Windows `pnpm` is a .cmd shim, which only a shell starts.
  const out =
    process.platform === "win32"
      ? spawnSync(quoted.join(" "), {
          cwd: options.cwd ?? root,
          env: { ...process.env, ...options.env },
          stdio: "inherit",
          shell: true,
          timeout: (options.minutes ?? 60) * 60_000,
        })
      : spawnSync(command[0] as string, command.slice(1), {
          cwd: options.cwd ?? root,
          env: { ...process.env, ...options.env },
          stdio: "inherit",
          timeout: (options.minutes ?? 60) * 60_000,
        });
  if (out.error) console.error(`perf-ui: ${out.error.message}`);
  return out.status ?? 1;
}

function must(command: readonly string[], options: Parameters<typeof run>[1] = {}): void {
  const status = run(command, options);
  if (status !== 0) {
    console.error(`perf-ui: ${command.slice(0, 3).join(" ")} … failed (exit ${status})`);
    process.exit(1);
  }
}

// The FFmpeg bundle is in the environment when `scripts/ffmpeg/env.sh` was sourced.
const ffmpegDir = process.env.FFMPEG_DIR;
const hasFfmpeg =
  ffmpegDir !== undefined && existsSync(path.join(ffmpegDir, "include", "libavcodec", "avcodec.h"));
const media = hasFfmpeg && !args.includes("--no-media");
const engine =
  process.env.CUTWEAVE_ENGINE_BIN ??
  path.join(root, "target", "release", exe("cutweave-engine-serve"));
const bench = path.join(root, "target", "release", exe("cutweave-bench"));

if (!args.includes("--no-build")) {
  if (process.env.CUTWEAVE_ENGINE_BIN === undefined) {
    must([
      ..."cargo build --release -p cutweave-engine --bin cutweave-engine-serve".split(" "),
      ...(hasFfmpeg ? ["--features", "ffmpeg"] : []),
    ]);
  }
  must("cargo build --release -p cutweave-bench".split(" "));
  must(["node", "scripts/ensure-electron.ts"], { cwd: desktopDir });
  must(["pnpm", "run", "build"], { cwd: desktopDir });
}
for (const file of [engine, bench]) {
  if (!existsSync(file)) {
    console.error(`perf-ui: ${file} is not built (run without --no-build)`);
    process.exit(1);
  }
}
console.info(
  media
    ? "perf-ui: real media (the engine decodes the projects' generated source files)"
    : `perf-ui: stub media (${hasFfmpeg ? "--no-media" : "no FFmpeg bundle in the environment: . scripts/ffmpeg/env.sh"})`,
);

// The projects: the documents are written again on every run (seconds), the sources once.
must([bench, "gen", "--preset", "all", ...(media ? ["--with-media"] : [])], { minutes: 30 });

const scratch = path.join(root, "target", "tmp", "perf-ui");
mkdirSync(scratch, { recursive: true });
const raw = path.join(scratch, "ui.json");
rmSync(raw, { force: true });
const lock = args.includes("--no-lock")
  ? []
  : ["python", path.join(root, "scripts", "gpu-lock.py"), "--"];
const hidden = ["python", path.join(root, "scripts", "hidden-desktop.py"), "--"];
const suite = run(
  [...lock, ...hidden, "pnpm", "exec", "playwright", "test", "-c", "playwright.perf.config.ts"],
  {
    cwd: desktopDir,
    env: {
      CUTWEAVE_ENGINE_BIN: engine,
      CUTWEAVE_PERF_OUT: raw,
      CUTWEAVE_PERF_MEDIA: media ? "1" : "0",
      CUTWEAVE_PERF_CPU_THROTTLE: String(slow),
    },
    minutes: 90,
  },
);
if (suite !== 0 || !existsSync(raw)) {
  console.error(`perf-ui: the benchmark run failed (exit ${suite}); nothing was measured`);
  process.exit(1);
}
const result: BenchResult = readResult(raw);
const notMeasured: string[] = [];

// The renderer alone on the same project: the model and lanes of P-5K as the editor builds them
// (`keyframes.p5k.test.ts` writes them), drawn by `packages/timeline/bench` in headless Chromium.
if (!args.includes("--no-renderer") && slow === 1) {
  const model = path.join(scratch, "p5k-timeline.json");
  const rendered = path.join(scratch, "timeline.json");
  rmSync(model, { force: true });
  rmSync(rendered, { force: true });
  const wrote = run(
    ["pnpm", "exec", "vitest", "run", "packages/editor/src/timeline/keyframes.p5k.test.ts"],
    { env: { CUTWEAVE_P5K_OUT: model }, minutes: 10 },
  );
  const status =
    wrote === 0 && existsSync(model)
      ? run(
          [
            ...lock,
            ...["node", path.join(root, "packages", "timeline", "bench", "run.mjs")],
            ...["--fixture", model, "--seconds", "20", "--min-fps", "0", "--out", rendered],
          ],
          { cwd: path.join(root, "packages", "timeline"), minutes: 15 },
        )
      : 1;
  if (status === 0 && existsSync(rendered)) {
    const renderer = readResult(rendered);
    result.metrics.push(...renderer.metrics);
    result.details = { ...result.details, renderer: renderer.details };
  } else {
    notMeasured.push(
      status === 3
        ? "the renderer alone (timeline.*): Playwright's Chromium is not installed"
        : "the renderer alone (timeline.*): its benchmark did not run",
    );
  }
}

if (slow > 1) result.suite = "ui-slowed";
const out = path.resolve(value("--out") ?? defaultResultPath(root, result));
writeResult(out, result);
const budgets = loadBudgets(root);
const over = failed(budgets, result);
console.info(`\n${table(budgets, result)}\nresult: ${out}`);
console.info(`machine: ${JSON.stringify(result.details)}`);
for (const line of notMeasured) console.info(`not measured: ${line}`);
if (over.length > 0) console.info(`over budget: ${over.join(", ")}`);
process.exit(over.length > 0 ? 1 : 0);
