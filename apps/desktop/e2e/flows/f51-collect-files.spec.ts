// SPDX-License-Identifier: GPL-3.0-or-later
// F51 Collect files (docs/user-flows.md, WP-2F.04): import the demo media into a saved project;
// File > Collect files…; type a folder name and choose a location (the OS folder picker is
// stubbed); the dialog says how many files and how much; Collect; the dialog follows the copy and
// says where it is. On disk the folder holds the project file and every media file. Then the
// originals go away (the media folder is renamed) and the collected project is opened: nothing is
// missing, no relink dialog, no Offline badge. Archive (zip) is WP-2F.04b and is not driven here.
// An image sequence (#208) comes along whole: three numbered pictures imported as one card.
import { existsSync, mkdirSync, readdirSync, renameSync, writeFileSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import {
  chooseMenu,
  createProject,
  DEMO_FILES,
  dirtyDot,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  mediaCards,
  pickFiles,
  profileDir,
  removeDir,
  shortcut,
  snapshot,
  workspace,
} from "./helpers.ts";

const dialog = (page: Page, title: string) => page.getByRole("dialog", { name: title });
/** The pictures of the image sequence the flow adds (one card, three files). */
const PICTURES = ["shot_0001.png", "shot_0002.png", "shot_0003.png"] as const;

/** Every file below `dir`, as paths relative to it with `/`. */
function filesBelow(dir: string, prefix = ""): string[] {
  return readdirSync(path.join(dir, prefix), { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory() ? filesBelow(dir, `${prefix}${entry.name}/`) : [`${prefix}${entry.name}`],
  );
}

test.describe("F51 collect files", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("a project is collected into one folder that opens without its originals", async () => {
    test.setTimeout(300_000);
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    try {
      const { app, page, close } = await launchFlowApp(work, home);
      try {
        await createProject(app, page, "Promo", work.dir);
        await importWithButton(
          app,
          page,
          DEMO_FILES.map((f) => work.file(f)),
        );
        await expect(mediaCards(page)).toHaveCount(DEMO_FILES.length);
        // Three numbered pictures, imported as one image sequence.
        mkdirSync(path.join(work.media, "render"));
        for (const name of PICTURES) writeFileSync(path.join(work.media, "render", name), name);
        await pickFiles(
          app,
          PICTURES.map((name) => path.join(work.media, "render", name)),
        );
        await page.getByRole("button", { name: "Import", exact: true }).click();
        await page.getByRole("button", { name: "Import as image sequence" }).click();
        await expect(mediaCards(page)).toHaveCount(DEMO_FILES.length + 1);
        await shortcut(app, "Control+S");
        await expect(dirtyDot(page)).toBeHidden();

        // File > Collect files…: the name is proposed; Collect waits for a location.
        await chooseMenu(page, app, "File", "Collect files…");
        const form = dialog(page, "Collect files");
        await expect(form).toBeVisible();
        const name = form.getByRole("textbox", { name: "Folder name" });
        await expect(name).toHaveValue("Promo collected");
        const collect = form.getByRole("button", { name: "Collect", exact: true });
        await expect(collect).toBeDisabled();
        await name.fill("Handover");
        const out = path.join(work.dir, "Out");
        mkdirSync(out);
        const asked = await pickFiles(app, [out]);
        await form.getByRole("button", { name: "Choose…" }).click();
        await expect(form.getByRole("textbox", { name: "Location" })).toHaveValue(out);
        expect((await asked()).at(-1)?.properties).toContain("openDirectory");
        // The estimate: how many files and how much.
        // (Every picture of the sequence is a file to copy.)
        const fileCount = DEMO_FILES.length + PICTURES.length;
        await expect(form.getByRole("status")).toContainText(`${fileCount} media files`);
        await snapshot(page, info, "estimate");
        await expect(collect).toBeEnabled();
        await collect.click();

        // The dialog follows the copy and says where it is.
        const done = dialog(page, "Files collected");
        await expect(done).toBeVisible({ timeout: 60_000 });
        const dest = path.join(out, "Handover");
        await expect(done).toContainText(dest);
        await snapshot(page, info, "collected");
        await done.getByRole("button", { name: "Close" }).click();
        await expect(done).toBeHidden();

        // On disk: the project file and every media file, nothing half-copied.
        const files = filesBelow(dest);
        expect(files).toContain("Handover.cutweave");
        for (const file of DEMO_FILES) expect(files).toContain(`media/media/${file}`);
        for (const name of PICTURES) expect(files).toContain(`media/render/${name}`);
        expect(files.filter((f) => f.endsWith(".part"))).toEqual([]);
        // The open project is the original, untouched: nothing unsaved.
        await expect(page).toHaveTitle("Promo — Cutweave");
        await expect(dirtyDot(page)).toBeHidden();

        // The originals go away; the collected project opens complete.
        renameSync(work.media, path.join(work.dir, "media-gone"));
        expect(existsSync(work.media)).toBe(false);
        await pickFiles(app, [dest]);
        await chooseMenu(page, app, "File", "Open project…");
        await expect(page).toHaveTitle("Handover — Cutweave");
        // (The copy is named after its folder.)
        await expect(mediaCards(page)).toHaveCount(DEMO_FILES.length + 1);
        await expect(mediaCards(page).getByText("Offline", { exact: true })).toHaveCount(0);
        await expect(page.getByRole("dialog")).toHaveCount(0);
      } finally {
        await close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
