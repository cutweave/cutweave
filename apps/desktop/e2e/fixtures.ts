// SPDX-License-Identifier: GPL-3.0-or-later
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  test as base,
  type ElectronApplication,
  _electron as electron,
  type Page,
} from "@playwright/test";

const desktopDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

interface Fixtures {
  app: ElectronApplication;
  page: Page;
}

/**
 * Launches the built app (`dist/`) with a throwaway HOME / user-data dir and the fake engine
 * (test-support) standing in for `cutweave engine`.
 */
export const test = base.extend<Fixtures>({
  // biome-ignore lint/correctness/noEmptyPattern: Playwright requires a destructured first argument
  app: async ({}, use) => {
    const home = mkdtempSync(path.join(tmpdir(), "cutweave-e2e-"));
    const app = await electron.launch({
      args: [...(process.platform === "linux" ? ["--no-sandbox"] : []), desktopDir],
      env: {
        ...process.env,
        HOME: home,
        USERPROFILE: home,
        CUTWEAVE_USER_DATA: path.join(home, "userData"),
        CUTWEAVE_ENGINE_FAKE: path.join(desktopDir, "test-support", "fake-engine.ts"),
      },
    });
    await use(app);
    await app.close();
    rmSync(home, { recursive: true, force: true });
  },
  page: async ({ app }, use) => {
    const page = await app.firstWindow();
    await page.waitForLoadState("domcontentloaded");
    await use(page);
  },
});

export { expect } from "@playwright/test";
