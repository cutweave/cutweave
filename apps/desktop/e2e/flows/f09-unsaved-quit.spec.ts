// SPDX-License-Identifier: GPL-3.0-or-later
// F9 Unsaved changes on quit (docs/user-flows.md; WP-1M.07's close guard), as a person does it:
// edit, quit with File > Exit, and answer the editor's "Save changes to …?" dialog. Cancel keeps
// the app and the edit; Save quits with the edit on disk; Discard quits without it. The same
// dialog handling is what every e2e teardown uses (`quitApp` in fixtures.ts), so a test that
// leaves a project dirty never leaves a window waiting for someone to click.
import { readFileSync } from "node:fs";
import type { ElectronApplication, Page } from "@playwright/test";
import { expect, quitApp, test, unsavedPrompt } from "../fixtures.ts";
import {
  chooseMenu,
  createProject,
  dirtyDot,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  type Workspace,
  windowDirty,
  workspace,
} from "./helpers.ts";

/** Media in the project document on disk. */
function mediaOnDisk(doc: string): number {
  const saved = JSON.parse(readFileSync(doc, "utf8")) as { media: Record<string, unknown> };
  return Object.keys(saved.media).length;
}

/** One edit (an import from the media bin); waits until the dot shows it and main knows. */
async function editUnsaved(app: ElectronApplication, page: Page, work: Workspace) {
  await importWithButton(app, page, [work.file("interview.mp4")]);
  await expect(dirtyDot(page)).toBeVisible();
  await expect.poll(() => windowDirty(app)).toBe(true);
}

test.describe("F9 unsaved changes on quit", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("the dialog asks; Cancel keeps the app and the edit; Save quits with the edit saved", async () => {
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      const doc = await createProject(app, page, "Unsaved quit", work.dir);
      expect(mediaOnDisk(doc)).toBe(0);
      await editUnsaved(app, page, work);

      // File > Exit: the editor asks instead of closing.
      await chooseMenu(page, app, "File", "Exit");
      const prompt = unsavedPrompt(page);
      await expect(prompt.dialog).toBeVisible();
      await expect(prompt.dialog).toHaveAccessibleName("Save changes to Unsaved quit?");
      await expect(prompt.save).toBeVisible();
      await expect(prompt.discard).toBeVisible();

      // Cancel: nothing closes and the edit is still unsaved.
      await prompt.cancel.click();
      await expect(prompt.dialog).toBeHidden();
      expect(app.process().exitCode).toBeNull();
      await expect(dirtyDot(page)).toBeVisible();
      expect(mediaOnDisk(doc)).toBe(0);

      // Quit again and Save: the app exits and the document on disk has the media.
      await quitApp(app, page, "save");
      expect(mediaOnDisk(doc)).toBe(1);
    } finally {
      await close();
      work.remove();
    }
  });

  test("Discard quits without writing the edit to the document", async () => {
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      const doc = await createProject(app, page, "Discard quit", work.dir);
      await editUnsaved(app, page, work);

      await quitApp(app, page, "discard");
      expect(mediaOnDisk(doc)).toBe(0);
    } finally {
      await close();
      work.remove();
    }
  });
});
