// SPDX-License-Identifier: GPL-3.0-or-later
// F8 Save, close, reopen (docs/user-flows.md): Ctrl+S, quit, relaunch, open the project from
// Recent. The reopened timeline is identical (ids included) and nothing is unsaved.
import { expect, test } from "../fixtures.ts";
import {
  chooseMenu,
  clickClip,
  clipAt,
  createProject,
  dirtyDot,
  dropCard,
  editorCaughtUp,
  freeMainTrack,
  hasRealEngine,
  importWithButton,
  inspector,
  item,
  launchFlowApp,
  layout,
  mediaCard,
  mediaCards,
  openRecent,
  profileDir,
  recoveryDialog,
  removeDir,
  sequence,
  shortcut,
  snapshot,
  timelineGeometry,
  windowDirty,
  workspace,
} from "./helpers.ts";

test.describe("F8 save, close, reopen", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("a saved project reopens from Recent exactly as it was", async () => {
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    try {
      const first = await launchFlowApp(work, home);
      const { app, page } = first;
      await freeMainTrack(app, page); // clips stay where they are dropped
      await createProject(app, page, "Reopen", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4"), work.file("logo.png")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
      await editorCaughtUp(page);
      // The logo at the playhead (0 s) with Enter: it gets a track of its own.
      await mediaCard(page, "logo.png").click();
      await page.keyboard.press("Enter");
      await expect.poll(async () => (await sequence(page)).tracks.length).toBe(3);
      await editorCaughtUp(page);
      const id = clipAt(await timelineGeometry(page), "V1", 1).id;
      await clickClip(page, id);
      await inspector(page).opacity.focus();
      await page.keyboard.press("Shift+ArrowLeft");
      await expect.poll(async () => (await item(page, id)).visual.opacity).toBe(0.9);
      await expect(dirtyDot(page)).toBeVisible();

      // Ctrl+S: the dot goes, and main knows quitting has nothing to ask.
      await shortcut(app, "Control+S");
      await expect(dirtyDot(page)).toBeHidden();
      await expect.poll(() => windowDirty(app)).toBe(false);
      const before = await sequence(page);
      await snapshot(page, info, "saved");

      // Quit with File > Exit: no question, the app just closes.
      const closed = app.waitForEvent("close");
      await chooseMenu(page, app, "File", "Exit");
      await closed;
      await first.close();

      // Relaunch on the same profile and open the project from Recent.
      const second = await launchFlowApp(work, home);
      try {
        await expect(second.page).toHaveTitle("Untitled — Cutweave");
        await openRecent(second.page, "Reopen.cutweave");
        await expect(second.page).toHaveTitle("Reopen — Cutweave");
        await expect(recoveryDialog(second.page).dialog).toBeHidden();
        await expect(dirtyDot(second.page)).toBeHidden();
        await expect(mediaCards(second.page)).toHaveCount(2);
        expect(await sequence(second.page)).toEqual(before);
        await editorCaughtUp(second.page);
        // The clip is on the timeline where it was, with its opacity.
        await clickClip(second.page, id);
        await expect(inspector(second.page).start).toHaveValue("00:00:01:00");
        await expect(inspector(second.page).opacity).toHaveAttribute("aria-valuetext", "90%");
        await snapshot(second.page, info, "reopened");
      } finally {
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
