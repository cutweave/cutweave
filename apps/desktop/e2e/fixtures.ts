// SPDX-License-Identifier: GPL-3.0-or-later
import { existsSync, mkdtempSync, rmSync } from "node:fs";
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

/**
 * The real engine for the suites that need one: `CUTWEAVE_ENGINE_BIN`, else the launch-protocol
 * binary (`cargo build -p cutweave-engine --bin cutweave-engine-serve`). Tests skip without it.
 */
export const realEngine =
  process.env.CUTWEAVE_ENGINE_BIN ??
  path.join(
    desktopDir,
    "..",
    "..",
    "target",
    "debug",
    process.platform === "win32" ? "cutweave-engine-serve.exe" : "cutweave-engine-serve",
  );
export const hasRealEngine = existsSync(realEngine);
/** `launchApp` env that runs the real engine instead of the fake one. */
export const REAL_ENGINE_ENV = { CUTWEAVE_ENGINE_FAKE: "", CUTWEAVE_ENGINE_BIN: realEngine };

declare global {
  /** The editor's e2e handle (packages/editor `engine/testHandle.ts`). */
  var __cutweaveEditor:
    | {
        call(method: string, params?: Record<string, unknown>): Promise<unknown>;
        state(): unknown;
      }
    | undefined;
}

/** What the editor's e2e handle reports (`window.__cutweaveEditor.state()`, WP-1L.03). */
export interface EditorState {
  connection: string;
  indicator: string;
  supervisorState: string | null;
  generation: number | null;
  blocked: string | null;
  noticeId: number | null;
  recoveredApplied: number | null;
  docRev: number;
  outbox: string[];
}

/** Waits until the editor installed its e2e handle (after `app.getInfo()` reports test hooks). */
export async function editorHandle(page: Page): Promise<void> {
  await page.waitForFunction(() => globalThis.__cutweaveEditor !== undefined);
}

export function editorState(page: Page): Promise<EditorState> {
  return page.evaluate(() => {
    const handle = globalThis.__cutweaveEditor;
    if (handle === undefined) throw new Error("no editor handle");
    return handle.state() as unknown as EditorState;
  });
}

/** A call over the editor's own engine connection: the outbox, resend and resync under test. */
export function editorCall<T = unknown>(
  page: Page,
  method: string,
  params: Record<string, unknown> = {},
): Promise<T> {
  return page.evaluate(
    ({ method, params }) => {
      const handle = globalThis.__cutweaveEditor;
      if (handle === undefined) throw new Error("no editor handle");
      return handle.call(method, params);
    },
    { method, params },
  ) as Promise<T>;
}

/** Like {@link editorCall}, resolving with the engine's error code instead of throwing. */
export function editorCallError(
  page: Page,
  method: string,
  params: Record<string, unknown> = {},
): Promise<string | null> {
  return page.evaluate(
    async ({ method, params }) => {
      try {
        await globalThis.__cutweaveEditor?.call(method, params);
        return null;
      } catch (error) {
        return (error as { code?: string }).code ?? String(error);
      }
    },
    { method, params },
  );
}

export interface LaunchedApp {
  app: ElectronApplication;
  page: Page;
  /** The throwaway HOME / USERPROFILE (user data under `userData/`); removed by `close()`. */
  home: string;
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
    home,
    mainLog,
    documents,
    close: async () => {
      // A test may already have closed the app to inspect what it left behind.
      await app.close().catch(() => {});
      rmSync(home, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
    },
  };
}

/**
 * SIGKILL the supervised engine (a simulated crash, WP-1L.03); resolves with the killed pid.
 * Main's supervisor then restarts it (0.5 s, 1 s, 2 s backoff; failed after a 4th crash in 60 s).
 */
export async function killEngine(app: ElectronApplication): Promise<number> {
  const pid = await app.evaluate(() => globalThis.__cutweaveTest?.killEngine() ?? null);
  if (pid === null) throw new Error("killEngine: no engine is running");
  return pid;
}

/** The supervisor's status as main sees it (`engine:status`). */
export function engineStatus(app: ElectronApplication) {
  return app.evaluate(() => globalThis.__cutweaveTest?.engineStatus() ?? null);
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
