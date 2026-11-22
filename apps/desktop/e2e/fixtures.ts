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

export const desktopDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

export interface LaunchedApp {
  app: ElectronApplication;
  page: Page;
  /** Main-process stdout/stderr lines (the `[security]` log of blocked actions lands here). */
  mainLog: string[];
  /** Document responses of the first window, in order, with their CSP response header. */
  documents: { url: string; csp: string }[];
  close(): Promise<void>;
}

/**
 * Launch the built app (`dist/`) with a throwaway HOME / user-data dir, the fake engine
 * (test-support) standing in for `cutweave engine`, and the main-process test hooks enabled.
 */
export async function launchApp(env: Record<string, string> = {}): Promise<LaunchedApp> {
  const home = mkdtempSync(path.join(tmpdir(), "cutweave-e2e-"));
  const app = await electron.launch({
    args: [...(process.platform === "linux" ? ["--no-sandbox"] : []), desktopDir],
    env: {
      ...process.env,
      HOME: home,
      USERPROFILE: home,
      CUTWEAVE_USER_DATA: path.join(home, "userData"),
      CUTWEAVE_ENGINE_FAKE: path.join(desktopDir, "test-support", "fake-engine.ts"),
      CUTWEAVE_TEST_HOOKS: "1",
      ...env,
    },
  });
  const mainLog: string[] = [];
  const collect = (chunk: Buffer) => mainLog.push(...chunk.toString("utf8").split(/\r?\n/));
  app.process().stdout?.on("data", collect);
  app.process().stderr?.on("data", collect);
  const page = await app.firstWindow();
  const documents: LaunchedApp["documents"] = [];
  page.on("response", (response) => {
    if (response.request().resourceType() !== "document") return;
    documents.push({
      url: response.url(),
      csp: response.headers()["content-security-policy"] ?? "",
    });
  });
  await page.waitForLoadState("domcontentloaded");
  return {
    app,
    page,
    mainLog,
    documents,
    close: async () => {
      await app.close();
      rmSync(home, { recursive: true, force: true });
    },
  };
}

interface Fixtures {
  launched: LaunchedApp;
  app: ElectronApplication;
  page: Page;
  mainLog: string[];
}

export const test = base.extend<Fixtures>({
  // biome-ignore lint/correctness/noEmptyPattern: Playwright requires a destructured first argument
  launched: async ({}, use) => {
    const launched = await launchApp();
    await use(launched);
    await launched.close();
  },
  app: async ({ launched }, use) => use(launched.app),
  page: async ({ launched }, use) => use(launched.page),
  mainLog: async ({ launched }, use) => use(launched.mainLog),
});

export { expect } from "@playwright/test";
