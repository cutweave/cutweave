// SPDX-License-Identifier: GPL-3.0-or-later
import { defineConfig } from "@playwright/test";

// The UI benchmarks (WP-2J.02, bench/README.md): `pnpm --filter @cutweave/desktop perf:ui` runs
// `e2e/perf/*.perf.ts` with this config. They are measurements, not tests of behaviour, so the
// e2e and user-flow runs (`playwright.config.ts`, `*.spec.ts`) never pick them up; their smoke
// run is `e2e/flows/perf-smoke.spec.ts`. One app at a time; a run takes several minutes.
export default defineConfig({
  testDir: "e2e/perf",
  testMatch: "**/*.perf.ts",
  globalSetup: ["./e2e/global-setup.ts", "./e2e/run-check.ts"],
  retries: 0,
  workers: 1,
  fullyParallel: false,
  timeout: 30 * 60_000,
  expect: { timeout: 30_000 },
  reporter: "list",
  outputDir: "../../test-results/desktop-perf",
});
