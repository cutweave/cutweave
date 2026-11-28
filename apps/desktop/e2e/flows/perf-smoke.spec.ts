// SPDX-License-Identifier: GPL-3.0-or-later
// The UI benchmarks' smoke run (WP-2J.02, bench/README.md): every probe of `e2e/perf/suite.ts`
// once, on the tiny generated project (`cargo xtask gen-bench-project --preset smoke`), with no
// thresholds. Not a user flow: it is here because this is the suite that runs the real app in
// the gate, so a change that breaks a probe (a renamed control, a removed diagnostic, a changed
// result format) fails here and not months later on a benchmark run. The numbers themselves are
// `pnpm --filter @cutweave/desktop perf:ui`.
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { expect, test } from "../fixtures.ts";
import { loadBudgets, readResult, table, writeResult } from "../perf/result.ts";
import { REQUIRED_IDS, readProject, repoRoot, runUiSuite, SMOKE } from "../perf/suite.ts";
import { hasRealEngine, removeDir } from "./helpers.ts";

test.describe("UI benchmarks, smoke run", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("every probe measures something and the result has the benchmark format", async () => {
    const info = test.info();
    // One start, one project, a second each of pan and zoom, one drag: well under this.
    test.setTimeout(8 * 60_000);
    const dir = mkdtempSync(path.join(tmpdir(), "cutweave-perf-smoke-"));
    try {
      // The project comes from the generator the real run uses (seeded, nothing committed).
      const made = spawnSync(
        "cargo",
        ["run", "--quiet", "--package", "xtask", "--", "gen-bench-project"].concat([
          "--preset",
          "smoke",
          "--out",
          dir,
        ]),
        { cwd: repoRoot, encoding: "utf8", timeout: 6 * 60_000 },
      );
      expect(made.status, `gen-bench-project: ${made.stderr}${made.error?.message ?? ""}`).toBe(0);
      const smoke = readProject(path.join(dir, "P-smoke"));
      expect(smoke.items).toBeGreaterThan(0);

      const lines: string[] = [];
      const result = await runUiSuite({
        scale: SMOKE,
        projects: { hour: smoke, stress: smoke },
        realMedia: false,
        cpuThrottle: 1,
        log: (line) => lines.push(line),
      });

      // The format of `bench/result.schema.json`: what `cargo xtask bench-check` reads.
      const file = info.outputPath("ui-smoke.json");
      writeResult(file, result);
      const read = readResult(file);
      const schema = JSON.parse(
        readFileSync(path.join(repoRoot, "bench", "result.schema.json"), "utf8"),
      ) as { required: string[]; properties: Record<string, unknown> };
      for (const key of schema.required) expect(read, key).toHaveProperty(key);
      for (const key of Object.keys(read)) expect(Object.keys(schema.properties)).toContain(key);
      expect(read.suite).toBe("ui-smoke");
      expect(read.toolchain.electron).toMatch(/^\d+\./);
      expect(read.details).toMatchObject({ media: "stub", cpuThrottle: 1 });
      expect(typeof read.details?.cpu).toBe("string");
      expect(typeof read.details?.gpu).toBe("string");

      // Every probe reported, with a real number from at least one sample.
      const ids = read.metrics.map((m) => m.id);
      for (const id of REQUIRED_IDS) expect(ids, id).toContain(id);
      expect(new Set(ids).size).toBe(ids.length);
      for (const m of read.metrics) {
        expect(Number.isFinite(m.value), `${m.id} = ${m.value}`).toBe(true);
        expect(m.value, m.id).toBeGreaterThanOrEqual(0);
        expect(m.samples, m.id).toBeGreaterThanOrEqual(1);
        expect(["ms", "fps", "MB", "KB"], m.id).toContain(m.unit);
      }
      // Times that cannot be zero if the probe saw what it claims to time.
      for (const id of ["app_start_ms", "project_open_1h_ui_ms", "ui_command_rtt_p95_ms"]) {
        expect(read.metrics.find((m) => m.id === id)?.value, id).toBeGreaterThan(0);
      }
      // The ids the budgets name exist in the budget file, in the units reported here.
      const budgets = loadBudgets(repoRoot);
      for (const id of [
        "app_start_ms",
        "project_open_1h_ui_ms",
        "ui_command_rtt_p95_ms",
        "timeline_panzoom_5000_fps",
        "timeline_panzoom_5000_frame_p95_ms",
      ]) {
        const budget = budgets.find((b) => b.id === id);
        expect(budget, `budget row ${id}`).toBeDefined();
        expect(read.metrics.find((m) => m.id === id)?.unit, id).toBe(budget?.unit);
      }
      // No thresholds here (a debug engine on a tiny project): the values, without verdicts, are
      // for whoever reads the run's output.
      console.info(`${lines.join("\n")}\n${table([], read)}`);
    } finally {
      removeDir(dir);
    }
  });
});
