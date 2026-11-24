// SPDX-License-Identifier: GPL-3.0-or-later
import { spawnSync } from "node:child_process";
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
  /** Timeline items in the editor's document. */
  items: number;
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

/** Apps launched and not yet closed; the auto fixture closes whatever a test leaves open. */
const openApps = new Set<LaunchedApp>();

/** How long quitting may take, prompt included, before it counts as a hang. */
const CLOSE_TIMEOUT_MS = 20_000;

/** Resolves true once `promise` settles, or false after `ms`. */
const settleWithin = <T>(promise: Promise<T>, ms: number): Promise<boolean> =>
  new Promise((resolve) => {
    const timer = setTimeout(() => resolve(false), ms);
    const done = () => {
      clearTimeout(timer);
      resolve(true);
    };
    promise.then(done, done);
  });

/**
 * Kill the app and everything it started (the engine included). Teardown uses it for a hung
 * app; the crash flows use it as fault injection (the whole app dying at once).
 */
export function killTree(pid: number | undefined): void {
  if (pid === undefined) return;
  if (process.platform === "win32") {
    spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore" });
    return;
  }
  spawnSync("pkill", ["-KILL", "-P", String(pid)], { stdio: "ignore" });
  try {
    process.kill(pid, "SIGKILL");
  } catch {
    // already gone
  }
}

/** What to answer when quitting asks about unsaved changes. */
export type UnsavedAnswer = "save" | "discard" | "cancel";

/** The editor's "Save changes to …?" prompt (WP-1M.07) and its buttons. */
export function unsavedPrompt(page: Page) {
  const dialog = page.getByRole("dialog", { name: /^Save changes to / });
  return {
    dialog,
    save: dialog.getByRole("button", { name: "Save", exact: true }),
    discard: dialog.getByRole("button", { name: "Discard changes" }),
    cancel: dialog.getByRole("button", { name: "Cancel" }),
  };
}

/**
 * Quit the app the way a person does: ask it to quit, and when the editor asks about unsaved
 * changes, press `answer` in that dialog ("discard" by default, so a test's leftovers don't
 * pile up; tests that care about the outcome pass "save"). Resolves once the app has exited.
 * An app that is still running after CLOSE_TIMEOUT_MS is a hang: it is killed (so no window is
 * left on screen) and the error fails the test.
 */
export async function quitApp(
  app: ElectronApplication,
  page: Page,
  answer: Exclude<UnsavedAnswer, "cancel"> = "discard",
  /** The main process's output (`LaunchedApp.mainLog`): its last lines go with a hang report. */
  mainLog: readonly string[] = [],
): Promise<void> {
  let child: ReturnType<ElectronApplication["process"]>;
  try {
    child = app.process();
  } catch {
    // Playwright disposes an application once it has closed: nothing is left to quit.
    return;
  }
  const exited = new Promise<void>((resolve) => {
    if (child.exitCode !== null || child.signalCode !== null) resolve();
    else child.once("exit", () => resolve());
  });
  // Quit, as File > Exit / Alt+F4 would; with unsaved changes main holds it and the editor asks.
  const started = Date.now();
  const at = () => `${((Date.now() - started) / 1000).toFixed(1)} s`;
  void app.close().catch(() => {});
  const prompt = unsavedPrompt(page);
  const deadline = started + CLOSE_TIMEOUT_MS;
  let answered = 0;
  let windowClosed: string | null = null;
  while (Date.now() < deadline) {
    if (await settleWithin(exited, 250)) return;
    if (page.isClosed()) {
      windowClosed ??= at();
      continue;
    }
    if (await prompt.dialog.isVisible().catch(() => false)) {
      answered += 1;
      await prompt[answer].click({ timeout: 2_000 }).catch(() => {});
    }
  }
  // What was on screen and still running, for the hunt (a hang is a bug, not something to retry).
  const dialogs = page.isClosed()
    ? `the window had closed (at ${windowClosed ?? "?"})`
    : await page
        .getByRole("dialog")
        .allInnerTexts()
        .then((texts) => (texts.length === 0 ? "no dialog open" : texts.join(" | ")))
        .catch(() => "unknown");
  const running = processTree(child.pid);
  // How late it is: a slow exit and a stuck one need different fixes.
  const late = (await settleWithin(exited, CLOSE_TIMEOUT_MS))
    ? `it exited at ${at()}`
    : `still running at ${at()}`;
  killTree(child.pid);
  throw new Error(
    `the app did not quit within ${CLOSE_TIMEOUT_MS / 1000} s (killed so no window stays open); ` +
      `answered the unsaved-changes prompt ${answered} time(s); on screen: ${dialogs}; ` +
      `processes: ${running}; ${late}` +
      (mainLog.length === 0 ? "" : `\nmain log (last lines):\n${mainLog.slice(-20).join("\n")}`),
  );
}

/** `name(pid)` of a process and its descendants (Windows; elsewhere "n/a"): hang diagnostics. */
function processTree(pid: number | undefined): string {
  if (pid === undefined || process.platform !== "win32") return "n/a";
  const out = spawnSync(
    "powershell",
    [
      "-NoProfile",
      "-NonInteractive",
      "-Command",
      'Get-CimInstance Win32_Process | ForEach-Object { "$($_.ProcessId) $($_.ParentProcessId) $($_.Name)" }',
    ],
    { encoding: "utf8", maxBuffer: 16 * 1024 * 1024 },
  );
  const rows = (out.stdout ?? "")
    .split(/\r?\n/)
    .map((line) => line.trim().split(" "))
    .filter((parts) => parts.length >= 3)
    .map(([id, parent, ...name]) => ({
      pid: Number(id),
      ppid: Number(parent),
      name: name.join(" "),
    }));
  const found: string[] = [];
  const walk = (id: number, depth: number): void => {
    for (const row of rows.filter((r) => r.ppid === id)) {
      found.push(`${row.name}(${row.pid})`);
      if (depth < 4) walk(row.pid, depth + 1);
    }
  };
  const root = rows.find((r) => r.pid === pid);
  walk(pid, 0);
  return `${root === undefined ? "the app process is gone" : `${root.name}(${pid})`}; children: ${
    found.join(", ") || "none"
  }`;
}

/**
 * Launch the built app (`dist/`) with a throwaway HOME / user-data dir, the fake engine
 * (test-support) standing in for `cutweave engine`, and the main-process test hooks enabled.
 */
export async function launchApp(
  env: Record<string, string> = {},
  /** Extra Chromium switches, e.g. `--force-device-scale-factor=1.5`. */
  switches: string[] = [],
  options: {
    /**
     * Reuse this HOME (and its user data) instead of a fresh one: a relaunch finds the settings,
     * recent projects and run-files the previous launch left. The caller removes it; `close()`
     * keeps it.
     */
    home?: string;
  } = {},
): Promise<LaunchedApp> {
  const home = options.home ?? mkdtempSync(path.join(tmpdir(), "cutweave-e2e-"));
  const ownsHome = options.home === undefined;
  const app = await electron.launch({
    args: [...(process.platform === "linux" ? ["--no-sandbox"] : []), ...switches, desktopDir],
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
  let closing: Promise<void> | null = null;
  const launched: LaunchedApp = {
    app,
    page,
    home,
    mainLog,
    documents,
    // Idempotent: a test's own `finally` and the auto fixture may both call it. A test may also
    // have closed the app already to inspect what it left behind.
    close: () => {
      closing ??= (async () => {
        openApps.delete(launched);
        try {
          await quitApp(app, page, "discard", mainLog);
        } finally {
          if (ownsHome) {
            rmSync(home, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
          }
        }
      })();
      return closing;
    },
  };
  openApps.add(launched);
  return launched;
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

export const test = base.extend<Fixtures & { closeLeftovers: undefined }>({
  // Runs after every test, also one that timed out or threw before its own cleanup: closes any
  // app the test launched and left open, so no window (or unsaved-changes prompt) outlives it.
  closeLeftovers: [
    // biome-ignore lint/correctness/noEmptyPattern: Playwright requires a destructured first argument
    async ({}, use) => {
      await use(undefined);
      await Promise.all([...openApps].map((launched) => launched.close()));
    },
    { auto: true },
  ],
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
