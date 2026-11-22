// SPDX-License-Identifier: GPL-3.0-or-later
import { defineConfig } from "@playwright/test";
import { BASE_URL, PORT, VIEWPORT } from "./visual/shared.ts";

const ci = Boolean(process.env.CI);

// Serves the static Storybook build (`pnpm storybook:build`) and drives Chromium against it.
export default defineConfig({
  testDir: "./visual",
  testMatch: "*.spec.ts",
  outputDir: "./test-results",
  fullyParallel: true,
  ...(ci ? { workers: 2 } : {}),
  retries: 0,
  timeout: 60_000,
  reporter: ci ? [["list"], ["html", { open: "never" }]] : "list",
  use: {
    baseURL: BASE_URL,
    viewport: VIEWPORT,
    deviceScaleFactor: 1,
    colorScheme: "dark",
    locale: "en-US",
    timezoneId: "UTC",
    reducedMotion: "reduce",
    launchOptions: {
      // Software rendering and stable text rasterization: no GPU, no LCD AA, no font hinting.
      args: [
        "--disable-gpu",
        "--disable-lcd-text",
        "--font-render-hinting=none",
        "--force-color-profile=srgb",
        "--hide-scrollbars",
      ],
    },
  },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
  webServer: {
    command: `pnpm exec vite preview --outDir storybook-static --host 127.0.0.1 --port ${PORT} --strictPort`,
    url: BASE_URL,
    reuseExistingServer: !ci,
    timeout: 60_000,
  },
});
