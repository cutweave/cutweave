// SPDX-License-Identifier: GPL-3.0-or-later
import { defineConfig } from "@playwright/test";

// Playwright-Electron e2e. Run `pnpm --filter @cutweave/desktop build` first (the app loads
// `dist/`). Windows are real, so tests run one at a time.
export default defineConfig({
  testDir: "e2e",
  testMatch: "**/*.spec.ts",
  // Fails the run once, with the reason, when the real engine binary cannot load.
  globalSetup: "./e2e/global-setup.ts",
  workers: 1,
  fullyParallel: false,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: process.env.CI ? [["list"], ["html", { open: "never" }]] : "list",
  outputDir: "../../test-results/desktop",
});
