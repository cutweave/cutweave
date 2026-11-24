// SPDX-License-Identifier: GPL-3.0-or-later
// F12 App crash and recovery (docs/user-flows.md): edit without saving, the whole app dies (the
// window and the engine at once: fault injection, a forced kill of the process tree), relaunch.
// Opening the project offers the unsaved edits back: Restore brings them, Discard keeps the
// project as last saved.
import type { ElectronApplication, Page } from "@playwright/test";
import { expect, killTree, test } from "../fixtures.ts";
import {
  clickClip,
  clipAt,
  createProject,
  dirtyDot,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  importWithButton,
  inspector,
  item,
  launchFlowApp,
  layout,
  mediaCards,
  openRecent,
  profileDir,
  recoveryDialog,
  removeDir,
  sequence,
  snapshot,
  timelineGeometry,
  toast,
  workspace,
} from "./helpers.ts";

/** The whole app dies at once, as in a power cut or a crash: nothing gets to save or ask. */
async function crash(app: ElectronApplication): Promise<void> {
  const child = app.process();
  const exited = new Promise<void>((resolve) => {
    if (child.exitCode !== null || child.signalCode !== null) resolve();
    else child.once("exit", () => resolve());
  });
  killTree(child.pid);
  await exited;
}

/** Media and clips the editor shows for the open project. */
async function contents(page: Page) {
  return { media: await mediaCards(page).count(), layout: await layout(page) };
}

test.describe("F12 app crash and recovery", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("after a crash, Restore brings the unsaved edits back; after another, Discard does not", async () => {
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    try {
      // Saved empty, then three edits that are never saved: an import, a drop, a rename.
      const first = await launchFlowApp(work, home);
      await createProject(first.app, first.page, "Crashy", work.dir);
      await importWithButton(first.app, first.page, [work.file("interview.mp4")]);
      await dropCard(first.page, "interview.mp4", "V1", 1);
      await expect.poll(async () => (await layout(first.page))[0]?.clips).toEqual([[1, 10]]);
      await editorCaughtUp(first.page);
      const id = clipAt(await timelineGeometry(first.page), "V1", 1).id;
      await clickClip(first.page, id);
      await inspector(first.page).name.fill("Before the crash");
      await first.page.keyboard.press("Enter");
      await expect.poll(async () => (await item(first.page, id)).name).toBe("Before the crash");
      await expect(dirtyDot(first.page)).toBeVisible();
      const edited = await sequence(first.page);
      await crash(first.app);
      await first.close();

      // Relaunch, open it from Recent: the editor offers the three edits back.
      const second = await launchFlowApp(work, home);
      await openRecent(second.page, "Crashy.cutweave");
      const recovery = recoveryDialog(second.page);
      await expect(recovery.dialog).toBeVisible();
      await expect(recovery.dialog).toContainText("Cutweave closed before Crashy was saved.");
      await expect(recovery.restore).toContainText("3 edits");
      await snapshot(second.page, info, "recovery offered");
      await recovery.restore.click();
      await expect(recovery.dialog).toBeHidden();
      await expect(toast(second.page, "Changes restored")).toContainText(
        "3 unsaved edits are back.",
      );
      // Back as they were, ids included, and still unsaved.
      expect(await sequence(second.page)).toEqual(edited);
      expect(await contents(second.page)).toEqual({
        media: 1,
        layout: [
          { track: "V1", clips: [[1, 10]] },
          { track: "A1", clips: [[1, 10]] },
        ],
      });
      expect((await item(second.page, id)).name).toBe("Before the crash");
      await expect(dirtyDot(second.page)).toBeVisible();
      await snapshot(second.page, info, "restored");

      // Crash again before saving; this time Discard: the project as it was last saved.
      await crash(second.app);
      await second.close();
      const third = await launchFlowApp(work, home);
      try {
        await openRecent(third.page, "Crashy.cutweave");
        const again = recoveryDialog(third.page);
        await expect(again.dialog).toBeVisible();
        await again.discard.click();
        await expect(again.dialog).toBeHidden();
        await expect(third.page).toHaveTitle("Crashy — Cutweave");
        await expect(third.page.getByText("No media yet")).toBeVisible();
        expect(await contents(third.page)).toEqual({
          media: 0,
          layout: [
            { track: "V1", clips: [] },
            { track: "A1", clips: [] },
          ],
        });
        await expect(dirtyDot(third.page)).toBeHidden();
      } finally {
        await third.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
