// SPDX-License-Identifier: GPL-3.0-or-later
// F49 Relink missing media (docs/user-flows.md, WP-2F.03): import the demo media into a saved
// project and quit; move the media folder somewhere else and rename one file; reopen the project:
// the relink dialog says how many files are missing and lists them, and the bin marks each card
// Offline; "Search a folder…" (the OS folder picker is stubbed) finds every file by its content,
// the renamed one too; the badges go; Ctrl+Z puts the old locations back in one step and Ctrl+Y
// relinks again; "Relink…" on a card opens the same dialog. The offline slate and the export
// refusal are F58 (WP-2F.03c).
import { mkdirSync, renameSync } from "node:fs";
import path from "node:path";
import type { Locator, Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import {
  chooseMenu,
  createProject,
  DEMO_FILES,
  dirtyDot,
  editorCaughtUp,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  mediaCard,
  mediaCards,
  openRecent,
  pickFiles,
  profileDir,
  removeDir,
  shortcut,
  snapshot,
  workspace,
} from "./helpers.ts";

const offlineChips = (page: Page): Locator =>
  mediaCards(page).getByText("Offline", { exact: true });
const relinkDialog = (page: Page, title: string | RegExp): Locator =>
  page.getByRole("dialog", { name: title });

test.describe("F49 relink missing media", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("a moved media folder is found again from the dialog, and undo puts it back", async () => {
    test.setTimeout(300_000);
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    try {
      const first = await launchFlowApp(work, home);
      {
        const { app, page } = first;
        await createProject(app, page, "Relink", work.dir);
        await importWithButton(
          app,
          page,
          DEMO_FILES.map((f) => work.file(f)),
        );
        await expect(mediaCards(page)).toHaveCount(DEMO_FILES.length);
        // Nothing is missing: no badge, no dialog.
        await expect(offlineChips(page)).toHaveCount(0);
        await expect(page.getByRole("dialog")).toHaveCount(0);
        await shortcut(app, "Control+S");
        await expect(dirtyDot(page)).toBeHidden();
        const closed = app.waitForEvent("close");
        await chooseMenu(page, app, "File", "Exit");
        await closed;
        await first.close();
      }

      // The footage moves to an archive folder, and one file gets a new name.
      const archive = path.join(work.dir, "Archive", "2026");
      mkdirSync(archive, { recursive: true });
      const moved = path.join(archive, "media");
      renameSync(work.media, moved);
      renameSync(path.join(moved, "interview.mp4"), path.join(moved, "interview final cut.mp4"));

      const second = await launchFlowApp(work, home);
      try {
        const { app, page } = second;
        await openRecent(page, "Relink.cutweave");
        await expect(page).toHaveTitle("Relink — Cutweave");

        // The dialog opens by itself, names the count and lists every file with where it was.
        const dialog = relinkDialog(page, `${DEMO_FILES.length} media files are missing`);
        await expect(dialog).toBeVisible();
        const rows = dialog.getByRole("list", { name: "Missing media" }).getByRole("listitem");
        await expect(rows).toHaveCount(DEMO_FILES.length);
        await expect(rows.filter({ hasText: "interview.mp4" })).toContainText(work.media);
        await snapshot(page, info, "missing");

        // Not now: the bin still says which cards are offline.
        await dialog.getByRole("button", { name: "Not now" }).click();
        await expect(dialog).toBeHidden();
        await expect(offlineChips(page)).toHaveCount(DEMO_FILES.length);

        // Relink… on a card opens the dialog again.
        await mediaCard(page, "music.wav").click({ button: "right" });
        await page.getByRole("menuitem", { name: "Relink…" }).click();
        await expect(dialog).toBeVisible();

        // Search a folder: the parent of where the files are now. Every file is found by its
        // content, the renamed one too.
        const asked = await pickFiles(app, [path.join(work.dir, "Archive")]);
        await dialog.getByRole("button", { name: "Search a folder…" }).click();
        const done = relinkDialog(page, "All media found");
        await expect(done).toBeVisible();
        await expect(done.getByRole("status")).toHaveText(`Found ${DEMO_FILES.length} files.`);
        expect((await asked()).at(-1)?.properties).toContain("openDirectory");
        await snapshot(page, info, "found");
        await done.getByRole("button", { name: "Done" }).click();
        await expect(done).toBeHidden();
        await expect(offlineChips(page)).toHaveCount(0);
        // The card of the renamed file shows the name the file has now.
        await expect(mediaCard(page, "interview final cut.mp4")).toBeVisible();
        await editorCaughtUp(page);

        // One undo step puts every old location back (the files are not there); redo relinks.
        await shortcut(app, "Control+Z");
        await expect(offlineChips(page)).toHaveCount(DEMO_FILES.length);
        await expect(page.getByRole("dialog")).toHaveCount(0);
        await shortcut(app, "Control+Y");
        await expect(offlineChips(page)).toHaveCount(0);
      } finally {
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
