// SPDX-License-Identifier: GPL-3.0-or-later
import { defineConfig } from "@playwright/test";

// Playwright-Electron e2e. Run `pnpm --filter @cutweave/desktop build` first (the app loads
// `dist/`). Windows are real, so tests run one at a time. The user-flow suite is `e2e/flows`
// (`just e2e-flows`, ADR-0018). No retries: a flaky test is a bug to fix. After the run,
// `e2e/run-check.ts` fails it if any app or engine process is still running.
export default defineConfig({
  testDir: "e2e",
  testMatch: "**/*.spec.ts",
  globalSetup: "./e2e/run-check.ts",
  retries: 0,
  workers: 1,
  fullyParallel: false,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  outputDir: "../../test-results/desktop",
});
