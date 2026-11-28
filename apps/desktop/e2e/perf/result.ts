// SPDX-License-Identifier: GPL-3.0-or-later
// The UI benchmarks' numbers (WP-2J.02): statistics, the result file and the budget table. The
// result is the format of `bench/result.schema.json`, the one `cutweave-bench` writes
// (`cutweave_bench::result`), so `cargo xtask bench-check <file>` judges a UI run like an engine
// run and the CI gate (WP-2J.04) reads both. The verdicts printed here follow the same rules
// (`budgets.toml` rows, `machine_scope`, the unit must match); `bench-check` stays the gate.
// Nothing here touches the app: `result.test.ts` runs it as an ordinary unit test.
import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";

/** One measured metric (`bench/result.schema.json`). */
export interface Metric {
  /** A budget id of `bench/budgets.toml`, or a name of its own for an informational metric. */
  id: string;
  value: number;
  unit: string;
  samples: number;
}

/** A benchmark run (`bench/result.schema.json`). */
export interface BenchResult {
  machine: string;
  commit: string;
  date: string;
  suite: string;
  toolchain: {
    rustc: string | null;
    ffmpeg: string | null;
    wgpu: string | null;
    electron: string | null;
  };
  metrics: Metric[];
  /** Where the numbers come from: the machine (CPU, GPU, memory) and how the run was set up. */
  details?: Record<string, unknown>;
}

/** A row of `bench/budgets.toml`. */
export interface Budget {
  id: string;
  metric: string;
  op: "<" | "<=" | ">=" | ">";
  value: number;
  unit: string;
  machine_scope: string[];
}

export type Verdict = "PASS" | "FAIL" | "skipped (other machines)" | "info";

/** A metric; `samples` is at least 1, as the schema wants. */
export const metric = (id: string, value: number, unit: string, samples: number): Metric => ({
  id,
  value: Math.round(value * 1000) / 1000,
  unit,
  samples: Math.max(1, Math.round(samples)),
});

/** The `q` quantile (0..1) of `values` (nearest rank); `NaN` when empty. */
export function quantile(values: readonly number[], q: number): number {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil(q * sorted.length) - 1))] ?? NaN;
}

export const median = (values: readonly number[]): number => quantile(values, 0.5);

/** What a run of frame intervals (ms) says. */
export interface FrameStats {
  frames: number;
  /** Frames per second over the whole run: frames divided by the time they took. */
  fps: number;
  p50: number;
  p95: number;
  max: number;
}

export function frameStats(intervals: readonly number[]): FrameStats {
  const total = intervals.reduce((sum, v) => sum + v, 0);
  return {
    frames: intervals.length,
    fps: total > 0 ? (1000 * intervals.length) / total : NaN,
    p50: quantile(intervals, 0.5),
    p95: quantile(intervals, 0.95),
    max: intervals.length === 0 ? NaN : Math.max(...intervals),
  };
}

/**
 * The rows of `bench/budgets.toml` (`[[budget]]`, `[[wp]]`, `[[synthetic]]`). The file is flat:
 * a table header, then `key = value` lines whose values are strings, numbers or string lists,
 * which read as JSON.
 */
export function parseBudgets(toml: string): Budget[] {
  const rows: Record<string, unknown>[] = [];
  for (const raw of toml.split(/\r?\n/)) {
    const line = raw.trim();
    if (line === "" || line.startsWith("#")) continue;
    if (/^\[\[[\w-]+\]\]$/.test(line)) {
      rows.push({});
      continue;
    }
    const pair = /^([\w-]+)\s*=\s*(.+)$/.exec(line);
    const row = rows.at(-1);
    if (pair === null || row === undefined) throw new Error(`budgets.toml: cannot read "${line}"`);
    row[pair[1] as string] = JSON.parse(pair[2] as string);
  }
  return rows.map((row) => {
    const { id, metric, op, value, unit, machine_scope } = row as Partial<Budget>;
    if (
      typeof id !== "string" ||
      typeof metric !== "string" ||
      (op !== "<" && op !== "<=" && op !== ">=" && op !== ">") ||
      typeof value !== "number" ||
      typeof unit !== "string" ||
      !Array.isArray(machine_scope)
    ) {
      throw new Error(`budgets.toml: incomplete row ${JSON.stringify(row)}`);
    }
    return { id, metric, op, value, unit, machine_scope };
  });
}

/** `<root>/bench/budgets.toml`. */
export const loadBudgets = (root: string): Budget[] =>
  parseBudgets(readFileSync(path.join(root, "bench", "budgets.toml"), "utf8"));

const holds = (op: Budget["op"], value: number, budget: number): boolean =>
  op === "<"
    ? value < budget
    : op === "<="
      ? value <= budget
      : op === ">="
        ? value >= budget
        : value > budget;

/** How `metric`, measured on `machine`, stands against its budget (`cutweave_bench::result`). */
export function verdict(budgets: readonly Budget[], machine: string, m: Metric): Verdict {
  const budget = budgets.find((b) => b.id === m.id);
  if (budget === undefined) return "info";
  if (!budget.machine_scope.some((s) => s === "any" || s === machine)) {
    return "skipped (other machines)";
  }
  return budget.unit === m.unit && holds(budget.op, m.value, budget.value) ? "PASS" : "FAIL";
}

/** Every metric with its budget and PASS or FAIL: what a run prints. */
export function table(budgets: readonly Budget[], result: BenchResult): string {
  const pad = (text: string, width: number) => text.padEnd(width);
  const lines = [
    `${pad("metric", 40)} ${"value".padStart(12)} ${pad("unit", 7)} ${pad("budget", 16)} result`,
  ];
  for (const m of result.metrics) {
    const budget = budgets.find((b) => b.id === m.id);
    const limit = budget === undefined ? "-" : `${budget.op} ${budget.value} ${budget.unit}`;
    lines.push(
      `${pad(m.id, 40)} ${m.value.toFixed(3).padStart(12)} ${pad(m.unit, 7)} ${pad(limit, 16)} ${verdict(budgets, result.machine, m)}`,
    );
  }
  return `${lines.join("\n")}\n`;
}

/** The metrics outside their budget. */
export const failed = (budgets: readonly Budget[], result: BenchResult): string[] =>
  result.metrics.filter((m) => verdict(budgets, result.machine, m) === "FAIL").map((m) => m.id);

/** First output line of `cmd args`, or null when it does not run. */
function toolVersion(cmd: string, args: readonly string[], cwd: string): string | null {
  try {
    const out = execFileSync(cmd, [...args], { cwd, encoding: "utf8", timeout: 20_000 });
    return out.split(/\r?\n/)[0]?.trim() ?? null;
  } catch {
    return null;
  }
}

/** The CPU, memory and system of this machine, for a result's `details`. */
export function hostInfo(): Record<string, unknown> {
  const cpus = os.cpus();
  return {
    cpu: cpus[0]?.model.trim() ?? "unknown",
    logicalCpus: cpus.length,
    memoryGb: Math.round((os.totalmem() / 2 ** 30) * 10) / 10,
    os: `${os.type()} ${os.release()} ${os.arch()}`,
  };
}

/**
 * A result for this machine (`CUTWEAVE_BENCH_MACHINE`, default `local`), the checkout at `root`
 * and today (UTC), as `BenchResult::now` makes one.
 */
export function buildResult(
  root: string,
  suite: string,
  metrics: Metric[],
  electron: string | null,
  details: Record<string, unknown>,
): BenchResult {
  return {
    machine: process.env.CUTWEAVE_BENCH_MACHINE ?? "local",
    commit: toolVersion("git", ["rev-parse", "HEAD"], root) ?? "unknown",
    date: new Date().toISOString().slice(0, 10),
    suite,
    toolchain: {
      rustc: toolVersion("rustc", ["--version"], root),
      ffmpeg: toolVersion("ffmpeg", ["-version"], root),
      wgpu: null,
      electron,
    },
    metrics,
    details: { ...hostInfo(), ...details },
  };
}

/**
 * `<root>/bench-results/{machine}/{date}.{suite}.json`: next to the engine suite's
 * `{date}.json`, where the CI gate (WP-2J.04) looks. A later run on the same day replaces it.
 */
export const defaultResultPath = (root: string, result: BenchResult): string =>
  path.join(root, "bench-results", result.machine, `${result.date}.${result.suite}.json`);

export function writeResult(file: string, result: BenchResult): void {
  mkdirSync(path.dirname(file), { recursive: true });
  writeFileSync(file, `${JSON.stringify(result, null, 2)}\n`);
}

export const readResult = (file: string): BenchResult =>
  JSON.parse(readFileSync(file, "utf8")) as BenchResult;
