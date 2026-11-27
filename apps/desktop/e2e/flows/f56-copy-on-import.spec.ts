// SPDX-License-Identifier: GPL-3.0-or-later
// F56 Copy on import (docs/user-flows.md, WP-2F.04c): Settings > Media and import > "Copy
// imported files into the project folder" is off; turn it on; create a project and import a file
// from outside its folder with the bin's Import button. The file is copied under
// `<project>/media/` and the card points at the copy. Save and quit, rename the original away,
// start again and reopen the project: nothing is missing (no relink dialog, no Offline badge).
import { existsSync, readFileSync, renameSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import {
  chooseMenu,
  closeSettings,
  createProject,
  dirtyDot,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  mediaCard,
  mediaCards,
  openRecent,
  openSettingsPage,
  profileDir,
  removeDir,
  shortcut,
  snapshot,
  toast,
  workspace,
} from "./helpers.ts";

/** Where the project says each media file is. */
async function mediaPaths(page: Page): Promise<(string | null)[]> {
  const out = await editorCall<{ media: { path: string | null }[] }>(page, "media.list", {});
  return out.media.map((m) => m.path);
}

test.describe("F56 copy on import", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("an imported file is copied into the project, which opens without the original", async () => {
    test.setTimeout(300_000);
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    const original = work.file("logo.png");
    const copy = path.join(work.dir, "Promo", "media", "media", "logo.png");
    try {
      const first = await launchFlowApp(work, home);
      {
        const { app, page } = first;
        // The setting is off until the person turns it on.
        const settings = await openSettingsPage(page, app, "Media and import");
        const copies = settings.getByRole("switch", {
          name: "Copy imported files into the project folder",
        });
        await expect(copies).not.toBeChecked();
        await copies.click();
        await expect(copies).toBeChecked();
        await snapshot(page, info, "setting");
        await closeSettings(settings);

        await createProject(app, page, "Promo", work.dir);
        await importWithButton(app, page, [original]);
        await expect(toast(page, "1 file added.").first()).toBeVisible();
        await expect(mediaCard(page, "logo.png")).toBeVisible();

        // On disk: the copy is in the project's media folder, in a folder named after the one
        // the file came from, and holds the original's bytes. The original is still there.
        expect(existsSync(copy)).toBe(true);
        expect(readFileSync(copy).equals(readFileSync(original))).toBe(true);
        expect(existsSync(`${copy}.part`)).toBe(false);
        expect(existsSync(original)).toBe(true);
        // The card points at the copy.
        expect(await mediaPaths(page)).toEqual([copy]);
        await snapshot(page, info, "imported");

        await shortcut(app, "Control+S");
        await expect(dirtyDot(page)).toBeHidden();
        const closed = app.waitForEvent("close");
        await chooseMenu(page, app, "File", "Exit");
        await closed;
        await first.close();
      }

      // The original goes away.
      renameSync(work.media, path.join(work.dir, "media-gone"));
      expect(existsSync(original)).toBe(false);

      const second = await launchFlowApp(work, home);
      try {
        const { page } = second;
        await openRecent(page, "Promo.cutweave");
        await expect(page).toHaveTitle("Promo — Cutweave");
        // Nothing is missing: the card is there, no Offline badge, no relink dialog.
        await expect(mediaCards(page)).toHaveCount(1);
        await expect(mediaCard(page, "logo.png")).toBeVisible();
        await expect(mediaCards(page).getByText("Offline", { exact: true })).toHaveCount(0);
        await expect(page.getByRole("dialog")).toHaveCount(0);
        expect(await mediaPaths(page)).toEqual([copy]);
        await snapshot(page, info, "reopened");
      } finally {
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
