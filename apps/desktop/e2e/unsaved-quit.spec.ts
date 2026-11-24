// SPDX-License-Identifier: GPL-3.0-or-later
// Quitting with unsaved changes (WP-1M.07's close guard), as a person does it: edit, quit, and
// answer the editor's "Save changes to …?" dialog. Cancel keeps the app and the edit; Save quits
// with the edit on disk; Discard quits without it. The same dialog handling is what every e2e
// teardown uses (`quitApp` in fixtures.ts), so a test that leaves a project dirty never leaves a
// window waiting for someone to click. Needs the real engine (`cutweave-engine-serve`).
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import type { ElectronApplication, Page } from "@playwright/test";
import {
  editorCall,
  editorHandle,
  editorState,
  expect,
  hasRealEngine,
  type LaunchedApp,
  launchApp,
  quitApp,
  REAL_ENGINE_ENV,
  test,
  unsavedPrompt,
} from "./fixtures.ts";

async function launchReal(): Promise<LaunchedApp> {
  const launched = await launchApp(REAL_ENGINE_ENV);
  await editorHandle(launched.page);
  await expect.poll(async () => (await editorState(launched.page)).connection).toBe("ready");
  return launched;
}

/** A saved project with its first sequence and the path of its `.cutweave` document. */
async function savedProject(page: Page, folder: string) {
  const { sequence } = await editorCall<{ sequence: string }>(page, "project.new", {
    name: "Unsaved quit",
  });
  const { path: doc } = await editorCall<{ path: string }>(page, "project.save_as", {
    path: folder,
  });
  return { sequence, doc };
}

/** Tracks of `sequence` in the document on disk. */
function tracksOnDisk(doc: string, sequence: string): number {
  const saved = JSON.parse(readFileSync(doc, "utf8")) as {
    sequences: Record<string, { tracks: Record<string, unknown> }>;
  };
  return Object.keys(saved.sequences[sequence]?.tracks ?? {}).length;
}

/** Makes one edit and waits until the dot shows it and main knows quitting must ask. */
async function editUnsaved(app: ElectronApplication, page: Page, sequence: string) {
  await editorCall(page, "track.add", { sequence, kind: "video" });
  await expect(page.getByTestId("dirty-dot")).toBeVisible();
  await expect
    .poll(() => app.evaluate(() => globalThis.__cutweaveTest?.windowDirty() ?? false))
    .toBe(true);
}

test.describe("quitting with unsaved changes", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");
  let work: string;
  test.beforeEach(() => {
    work = mkdtempSync(path.join(tmpdir(), "cutweave-unsaved-"));
  });
  test.afterEach(() => {
    rmSync(work, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
  });

  test("the dialog asks; Cancel keeps the app and the edit; Save quits with the edit saved", async () => {
    const { app, page, close } = await launchReal();
    try {
      const { sequence, doc } = await savedProject(page, path.join(work, "cancel-then-save"));
      const before = tracksOnDisk(doc, sequence);
      await editUnsaved(app, page, sequence);

      // Quit (File > Exit): the editor asks instead of closing.
      await app.evaluate(({ app }) => app.quit());
      const prompt = unsavedPrompt(page);
      await expect(prompt.dialog).toBeVisible();
      await expect(prompt.save).toBeVisible();
      await expect(prompt.discard).toBeVisible();

      // Cancel: nothing closes and the edit is still unsaved.
      await prompt.cancel.click();
      await expect(prompt.dialog).toBeHidden();
      expect(app.process().exitCode).toBeNull();
      await expect(page.getByTestId("dirty-dot")).toBeVisible();
      expect(tracksOnDisk(doc, sequence)).toBe(before);

      // Quit again and Save: the app exits and the document on disk has the new track.
      await quitApp(app, page, "save");
      expect(tracksOnDisk(doc, sequence)).toBe(before + 1);
    } finally {
      await close();
    }
  });

  test("Discard quits without writing the edit to the document", async () => {
    const { app, page, close } = await launchReal();
    try {
      const { sequence, doc } = await savedProject(page, path.join(work, "discard"));
      const before = tracksOnDisk(doc, sequence);
      await editUnsaved(app, page, sequence);

      await quitApp(app, page, "discard");
      expect(tracksOnDisk(doc, sequence)).toBe(before);
    } finally {
      await close();
    }
  });
});
