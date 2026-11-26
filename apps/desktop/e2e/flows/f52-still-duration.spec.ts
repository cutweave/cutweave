// SPDX-License-Identifier: GPL-3.0-or-later
// F52 Still image duration (docs/user-flows.md, WP-2F.05b, ADR-0035): Settings > Editing sets how
// long a still is in new projects. Set it to 3 seconds, create a project, add a picture to the
// timeline with Enter: the clip is 3 s. Set it to 8 seconds and add the picture again: still 3 s,
// because the project keeps the value it was created with, and the first clip is unchanged. A
// second project, created now, gives 8 s. Changing an open project's own value is File > Project
// settings… (WP-2H.08) and is not driven here.
import type { ElectronApplication, Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import {
  closeSettings,
  createProject,
  dirtyDot,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  mediaCard,
  openSettingsPage,
  profileDir,
  removeDir,
  sec,
  sequence,
  shortcut,
  workspace,
} from "./helpers.ts";

/** Settings > Editing > Still image duration, typed and committed with Enter. */
async function setStillDuration(page: Page, app: ElectronApplication, seconds: number) {
  const settings = await openSettingsPage(page, app, "Editing");
  const field = settings.getByRole("spinbutton", { name: "Still image duration" });
  await field.fill(String(seconds));
  await field.press("Enter");
  await expect(field).toHaveValue(String(seconds));
  await closeSettings(settings);
}

/** The lengths of every clip of the sequence, in flicks, by start time. */
async function clipLengths(page: Page): Promise<number[]> {
  const seq = await sequence(page);
  return seq.tracks
    .flatMap((t) => t.items)
    .sort((a, b) => a.start - b.start)
    .map((i) => i.duration);
}

test.describe("F52 still image duration", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("new projects take the setting; a project and its clips keep theirs", async () => {
    test.setTimeout(240_000);
    const work = workspace();
    const home = profileDir();
    try {
      const { app, page, close } = await launchFlowApp(work, home);
      try {
        await setStillDuration(page, app, 3);
        await createProject(app, page, "Three", work.dir);
        await importWithButton(app, page, [work.file("logo.png")]);
        await mediaCard(page, "logo.png").click();
        await page.keyboard.press("Enter");
        await expect.poll(() => clipLengths(page)).toEqual([sec(3)]);

        // The setting is for new projects: this one keeps 3 s, and so does its first clip.
        await setStillDuration(page, app, 8);
        await mediaCard(page, "logo.png").click();
        await page.keyboard.press("Enter");
        await expect.poll(async () => (await clipLengths(page)).length).toBe(2);
        expect(await clipLengths(page)).toEqual([sec(3), sec(3)]);

        // A project created now takes 8 s.
        await shortcut(app, "Control+S");
        await expect(dirtyDot(page)).toBeHidden();
        await createProject(app, page, "Eight", work.dir);
        await importWithButton(app, page, [work.file("logo.png")]);
        await mediaCard(page, "logo.png").click();
        await page.keyboard.press("Enter");
        await expect.poll(() => clipLengths(page)).toEqual([sec(8)]);
      } finally {
        await close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
