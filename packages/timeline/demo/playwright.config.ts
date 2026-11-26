// SPDX-License-Identifier: GPL-3.0-or-later
// The timeline demo's perf test (`pnpm --filter @cutweave/timeline test:perf`): serves demo/ with
// Vite and drives its scripted sweep in Chromium. Timing on a shared machine: retried twice.
import { fileURLToPath } from "node:url";
import { defineConfig } from "@playwright/test";

const PORT = 5179;
const root = fileURLToPath(new URL("..", import.meta.url));

export default defineConfig({
  testDir: ".",
  // Not `*.spec.ts`: Vitest run in this package must not pick a Playwright test up.
  testMatch: "*.pw.ts",
  outputDir: "../test-results",
  retries: 2,
  workers: 1,
  timeout: 120_000,
  reporter: "list",
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    viewport: { width: 1600, height: 900 },
    deviceScaleFactor: 1,
    colorScheme: "dark",
    launchOptions: { args: ["--enable-precise-memory-info"] },
  },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
  webServer: {
    command: `pnpm exec vite demo --host 127.0.0.1 --port ${PORT} --strictPort`,
    cwd: root,
    url: `http://127.0.0.1:${PORT}`,
    reuseExistingServer: false,
    timeout: 60_000,
  },
});
