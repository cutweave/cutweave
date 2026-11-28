// SPDX-License-Identifier: GPL-3.0-or-later
// The UI benchmark run (WP-2J.02): `pnpm --filter @cutweave/desktop perf:ui` (scripts/perf-ui.ts
// builds what it needs, generates the projects and starts this). Writes the result where
// `CUTWEAVE_PERF_OUT` says; the script prints the table and decides pass or fail.
import { existsSync } from "node:fs";
import path from "node:path";
import { hasRealEngine, realEngine, test } from "../fixtures.ts";
import { writeResult } from "./result.ts";
import { FULL, readProject, repoRoot, runUiSuite } from "./suite.ts";

const projectsDir =
  process.env.CUTWEAVE_PERF_PROJECTS ?? path.join(repoRoot, "testdata", "bench-projects");
const project = (name: string) => path.join(projectsDir, name);

test("UI benchmarks on P-400 and P-5K", async () => {
  test.skip(!hasRealEngine, `no engine at ${realEngine}`);
  for (const name of ["P-400", "P-5K", "P-short"]) {
    if (!existsSync(path.join(project(name), "bench-fixture.json"))) {
      throw new Error(`${project(name)} is not generated (cargo xtask gen-bench-project)`);
    }
  }
  const out = process.env.CUTWEAVE_PERF_OUT;
  if (out === undefined) throw new Error("CUTWEAVE_PERF_OUT is not set (use scripts/perf-ui.ts)");
  const result = await runUiSuite({
    scale: FULL,
    projects: {
      hour: readProject(project("P-400")),
      stress: readProject(project("P-5K")),
      other: readProject(project("P-short")),
    },
    realMedia: process.env.CUTWEAVE_PERF_MEDIA === "1",
    cpuThrottle: Number(process.env.CUTWEAVE_PERF_CPU_THROTTLE ?? 1),
    log: (line) => console.info(`[perf] ${line}`),
  });
  writeResult(out, result);
});
