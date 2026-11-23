// SPDX-License-Identifier: GPL-3.0-or-later
// WP-1L.03 renderer half: the editor keeps working across engine restarts. Commands go through
// the editor's own connection (its e2e handle), so the outbox, the resend under the original
// request_id, the snapshot resync, the notice and the blocked states are the real ones. Most
// tests need the real engine (`cutweave-engine-serve`); they skip without it.

import { mkdtempSync, rmSync } from "node:fs";
import { createServer, type Server } from "node:net";
import { tmpdir } from "node:os";
import path from "node:path";
import type { ElectronApplication, Page } from "@playwright/test";
import {
  editorCall,
  editorCallError,
  editorHandle,
  editorState,
  engineStatus,
  expect,
  hasRealEngine,
  killEngine,
  type LaunchedApp,
  launchApp,
  REAL_ENGINE_ENV,
  test,
} from "./fixtures.ts";

const chip = (page: Page) => page.locator(".cw-engine-status");
/** The restart notice (the toast itself, not Radix's live-region copy of its text). */
const notice = (page: Page) => page.locator(".cw-toast", { hasText: "Engine restarted" });

/** Waits until main's engine `generation` is ready and the editor is connected to it again. */
async function backOnline(app: ElectronApplication, page: Page, generation: number) {
  await expect
    .poll(async () => (await engineStatus(app))?.generation === generation, { timeout: 20_000 })
    .toBe(true);
  await expect
    .poll(async () => {
      const s = await editorState(page).catch(() => null);
      return s?.connection === "ready" && s.generation === generation;
    })
    .toBe(true);
}

async function launchReal(env: Record<string, string> = {}): Promise<LaunchedApp> {
  const launched = await launchApp({ ...REAL_ENGINE_ENV, ...env });
  await editorHandle(launched.page);
  await expect.poll(async () => (await editorState(launched.page)).connection).toBe("ready");
  return launched;
}

/** A saved project (so the journal records every commit) with its first sequence. */
async function newProject(page: Page, folder: string): Promise<{ sequence: string; doc: string }> {
  const { sequence } = await editorCall<{ sequence: string }>(page, "project.new", {
    name: "Resync",
  });
  const { path: doc } = await editorCall<{ path: string }>(page, "project.save_as", {
    path: folder,
  });
  return { sequence, doc };
}

const trackCount = async (page: Page, sequence: string) =>
  (await editorCall<{ track_count: number }>(page, "timeline.get", { sequence })).track_count;

const addTrack = (page: Page, sequence: string) =>
  editorCall(page, "track.add", { sequence, kind: "video" });

test.describe("with the real engine", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("three edits survive a SIGKILL: reconnecting shown, recovered in < 5 s, each applied once", async () => {
    const { app, page, home, close } = await launchReal();
    try {
      const { sequence } = await newProject(page, path.join(home, "film"));
      for (let i = 0; i < 3; i++) await addTrack(page, sequence);
      expect(await trackCount(page, sequence)).toBe(5); // V1 + A1 + 3
      await expect(chip(page)).toHaveText("Engine connected");

      const killedAt = Date.now();
      await killEngine(app);
      await expect(chip(page)).toHaveText("Reconnecting");
      await expect(notice(page)).toContainText("Engine restarted — recovered");
      const recoveredIn = Date.now() - killedAt;
      expect(recoveredIn).toBeLessThan(5_000);
      await backOnline(app, page, 2);
      // Main replayed the journal (3 records, fewer if autosave ran first under load).
      await expect(notice(page)).toContainText("film.cutweave");
      await expect(chip(page)).toHaveText("Engine connected");
      // Exactly once: the journal brought back three tracks, nothing doubled.
      expect(await trackCount(page, sequence)).toBe(5);
      // The notice goes away on its own after 3 s.
      await expect(notice(page)).toBeHidden({ timeout: 6_000 });
    } finally {
      await close();
    }
  });

  test("a command fired during the outage is applied exactly once (10 times)", async () => {
    test.setTimeout(180_000);
    const { app, page, home, close } = await launchReal();
    try {
      const { sequence } = await newProject(page, path.join(home, "outage"));
      for (let round = 0; round < 10; round++) {
        await app.evaluate(() => globalThis.__cutweaveTest?.resetEngineCrashCount());
        const before = await trackCount(page, sequence);
        await killEngine(app);
        // Fired while the engine is down: it waits in the outbox, then goes out once.
        const added = addTrack(page, sequence);
        await backOnline(app, page, round + 2);
        await added;
        expect(await trackCount(page, sequence)).toBe(before + 1);
        expect((await editorState(page)).outbox).toEqual([]);
      }
    } finally {
      await close();
    }
  });

  test("a command the engine committed but never answered is applied exactly once (10 times)", async () => {
    test.setTimeout(240_000);
    // The project is made first, without the crash hook (making it takes commits too).
    const folder = mkdtempSync(path.join(tmpdir(), "cutweave-e2e-project-"));
    try {
      const first = await launchReal();
      const { doc, sequence } = await newProject(first.page, path.join(folder, "unacked"));
      await first.close();

      // Every engine now aborts right after its first commit, before answering it.
      const { app, page, close } = await launchReal({ CUTWEAVE_TEST_CRASH_AFTER: "1" });
      try {
        await editorCall(page, "project.open", { path: doc });
        await expect
          .poll(() => app.evaluate(() => globalThis.__cutweaveTest?.engineProject() ?? null))
          .toBe(doc);
        for (let round = 0; round < 10; round++) {
          await app.evaluate(() => globalThis.__cutweaveTest?.resetEngineCrashCount());
          const before = await trackCount(page, sequence);
          // Committed and journaled, never answered: the engine aborts, main restarts and
          // recovers it, the editor re-sends it under the same request_id and the engine
          // answers from its deduplication window.
          await addTrack(page, sequence);
          await backOnline(app, page, round + 2);
          // The unanswered commit was journaled, so main had at least it to restore.
          expect((await editorState(page)).recoveredApplied).toBeGreaterThanOrEqual(1);
          expect(await trackCount(page, sequence)).toBe(before + 1);
        }
      } finally {
        await close();
      }
    } finally {
      rmSync(folder, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
    }
  });

  test("a restart on a new port reloads the editor, which connects there", async () => {
    const { app, page, documents, close } = await launchReal();
    let blocker: Server | null = null;
    try {
      const oldPort = (await app.evaluate(() => globalThis.__cutweaveTest?.enginePort())) ?? 0;
      await killEngine(app);
      // Take the engine's port before the 0.5 s backoff ends (the OS frees it with the process).
      blocker = await new Promise<Server>((resolve, reject) => {
        const deadline = Date.now() + 400;
        const attempt = () => {
          const server = createServer();
          server.once("error", () =>
            Date.now() < deadline ? setTimeout(attempt, 20) : reject(new Error("port stays busy")),
          );
          server.listen(oldPort, "127.0.0.1", () => resolve(server));
        };
        attempt();
      });
      await expect
        .poll(async () => (await engineStatus(app))?.generation, { timeout: 20_000 })
        .toBe(2);
      const newPort = await app.evaluate(() => globalThis.__cutweaveTest?.enginePort());
      expect(newPort).not.toBe(oldPort);
      // Main reloaded the editor with a CSP for the new port; the fresh editor connects there.
      await expect
        .poll(() => documents.at(-1)?.csp ?? "")
        .toContain(`connect-src http://127.0.0.1:${newPort} ws://127.0.0.1:${newPort};`);
      await editorHandle(page);
      await expect.poll(async () => (await editorState(page)).connection).toBe("ready");
      await expect(chip(page)).toHaveText("Engine connected");
      expect(await editorCall(page, "project.info")).toBeTruthy();
    } finally {
      blocker?.close();
      await close();
    }
  });

  test("closing the app leaves no engine process behind", async () => {
    const { app, close } = await launchReal();
    const pid = await app.evaluate(() => globalThis.__cutweaveTest?.enginePid() ?? null);
    expect(pid).not.toBeNull();
    await close();
    const alive = () => {
      try {
        process.kill(pid ?? 0, 0);
        return true;
      } catch {
        return false;
      }
    };
    await expect.poll(alive, { timeout: 10_000 }).toBe(false);
  });
});

test("a fourth crash in 60 s blocks editing until the dialog's Restart", async () => {
  const { app, page, close } = await launchApp();
  try {
    await app.evaluate(() => globalThis.__cutweaveTest?.captureEngineDialog());
    await editorHandle(page);
    await expect.poll(async () => (await editorState(page)).connection).toBe("ready");
    for (let generation = 2; generation <= 4; generation++) {
      await killEngine(app);
      await backOnline(app, page, generation);
    }
    await killEngine(app);
    await expect.poll(async () => (await engineStatus(app))?.state).toBe("failed");
    // Main's dialog was asked for, and the editor shows why nothing can be edited.
    expect(await app.evaluate(() => globalThis.__cutweaveTest?.engineDialogs().length)).toBe(1);
    await expect(page.getByRole("dialog", { name: "The engine stopped" })).toBeVisible();
    await expect(chip(page)).toHaveText("Engine stopped");
    expect(await editorCallError(page, "project.rename", { name: "x" })).toBe("unsupported");

    await app.evaluate(() => globalThis.__cutweaveTest?.answerEngineDialog("restart"));
    await backOnline(app, page, 5);
    await expect(page.getByRole("dialog", { name: "The engine stopped" })).toBeHidden();
    await expect(chip(page)).toHaveText("Engine connected");
  } finally {
    await close();
  }
});

test("an engine with another schema hash is refused: editing is blocked", async () => {
  const { page, close } = await launchApp({ FAKE_ENGINE_SCHEMA_HASH: "0".repeat(64) });
  try {
    await editorHandle(page);
    await expect(
      page.getByRole("dialog", { name: "This editor does not match the engine" }),
    ).toBeVisible();
    await expect(chip(page)).toHaveText("Engine mismatch");
    expect((await editorState(page)).blocked).toBe("schema-mismatch");
    expect(await editorCallError(page, "project.rename", { name: "x" })).toBe("unsupported");
  } finally {
    await close();
  }
});
