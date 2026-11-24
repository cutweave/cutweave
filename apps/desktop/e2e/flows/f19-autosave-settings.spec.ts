// SPDX-License-Identifier: GPL-3.0-or-later
// F19 Autosave settings and the save status (docs/user-flows.md; WP-1M.10), as a person does it:
// File > Settings… > Saving, edits, the title bar's save status, File > Save and Ctrl+S, quit and
// relaunch. The title bar has no Save button; its status icon says whether everything is saved.
import { readFileSync } from "node:fs";
import type { Locator, Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import {
  chooseMenu,
  closeSettings,
  createProject,
  dirtyDot,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  openRecent,
  openSettings,
  profileDir,
  removeDir,
  saveStatus,
  shortcut,
  snapshot,
  toolbar,
  workspace,
} from "./helpers.ts";

/** Media in the project document on disk. */
function mediaOnDisk(doc: string): number {
  const saved = JSON.parse(readFileSync(doc, "utf8")) as { media: Record<string, unknown> };
  return Object.keys(saved.media).length;
}

/**
 * How long a negative is watched: the idle save fires 2 s after the last edit, and it is the
 * first thing an enabled autosave does, so a document still unsaved after this was not saved by
 * autosave. (The interval save checks the same switch.)
 */
const WATCH_MS = 6000;

/** Fails if the status says "All changes saved" within `WATCH_MS`: the wait for a non-event. */
async function staysUnsaved(page: Page): Promise<void> {
  const saved = page.locator("[data-testid='save-status'][data-state='saved']");
  const appeared = await expect(saved)
    .toBeVisible({ timeout: WATCH_MS })
    .then(
      () => true,
      () => false,
    );
  expect(appeared, "the project was saved although autosave is off").toBe(false);
}

/** Waits for the dialog's opening animation to end, so a screenshot shows it as it rests. */
const opaque = (dialog: Locator): Promise<void> =>
  expect.poll(() => dialog.evaluate((el) => getComputedStyle(el).opacity)).toBe("1");

const radio = (group: Locator, name: string): Locator => group.getByRole("radio", { name });

test.describe("F19 autosave settings and the save status", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("autosave off keeps edits unsaved until Ctrl+S; on saves them; the choice survives a relaunch", async () => {
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    try {
      const first = await launchFlowApp(work, home);
      const { app, page } = first;
      const doc = await createProject(app, page, "Autosave", work.dir);

      // Saving is automatic: no Save button in the title bar; the status says all is saved.
      await expect(toolbar(page).getByRole("button", { name: /^Save/ })).toHaveCount(0);
      await expect(page.getByRole("button", { name: /^Save/ })).toHaveCount(0);
      await expect(saveStatus(page)).toHaveText("All changes saved");
      await expect(dirtyDot(page)).toBeHidden();

      // Settings > Saving: on, every 30 s and 10 backups by default. Turn it off.
      let settings = await openSettings(page, app);
      const automatic = settings.getByRole("switch", { name: "Save automatically" });
      const interval = settings.getByRole("radiogroup", { name: "Save at least every" });
      const backups = settings.getByRole("radiogroup", { name: "Backups to keep" });
      await expect(automatic).toBeChecked();
      await expect(radio(interval, "30 s")).toHaveAttribute("aria-checked", "true");
      await expect(radio(backups, "10")).toHaveAttribute("aria-checked", "true");
      await opaque(settings);
      await snapshot(page, info, "settings saving dark");
      const theme = settings.getByRole("radiogroup", { name: "Theme" });
      await radio(theme, "Light").click();
      await expect(radio(theme, "Light")).toHaveAttribute("aria-checked", "true");
      await opaque(settings);
      await snapshot(page, info, "settings saving light");
      await radio(theme, "Dark").click();
      await expect(radio(theme, "Dark")).toHaveAttribute("aria-checked", "true");
      await automatic.click();
      await expect(automatic).not.toBeChecked();
      await expect(radio(interval, "5 min")).toBeDisabled();
      await closeSettings(settings);

      // An edit stays unsaved: the status says so, and the file on disk does not change.
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await expect(dirtyDot(page)).toBeVisible();
      await expect(saveStatus(page)).toHaveText("Unsaved changes, autosave is off");
      await snapshot(page, info, "unsaved, autosave off");
      await staysUnsaved(page);
      await expect(dirtyDot(page)).toBeVisible();
      expect(mediaOnDisk(doc)).toBe(0);

      // Turn it on at 30 s: the pending edit is saved.
      settings = await openSettings(page, app);
      await automatic.click();
      await expect(automatic).toBeChecked();
      await radio(interval, "30 s").click();
      await expect(radio(interval, "30 s")).toHaveAttribute("aria-checked", "true");
      await closeSettings(settings);
      await expect(saveStatus(page)).toHaveText("All changes saved");
      await expect(dirtyDot(page)).toBeHidden();
      expect(mediaOnDisk(doc)).toBe(1);
      await snapshot(page, info, "saved");

      // Off again. File > Save and Ctrl+S still save, with no button.
      settings = await openSettings(page, app);
      await automatic.click();
      await expect(automatic).not.toBeChecked();
      await closeSettings(settings);
      await importWithButton(app, page, [work.file("logo.png")]);
      await expect(saveStatus(page)).toHaveText("Unsaved changes, autosave is off");
      await chooseMenu(page, app, "File", "Save");
      await expect(saveStatus(page)).toHaveText("All changes saved");
      expect(mediaOnDisk(doc)).toBe(2);
      await importWithButton(app, page, [work.file("music.wav")]);
      await expect(dirtyDot(page)).toBeVisible();
      await shortcut(app, "Control+S");
      await expect(dirtyDot(page)).toBeHidden();
      expect(mediaOnDisk(doc)).toBe(3);

      // Choose the interval and backups while it is on (they are disabled while it is off), then
      // turn it off for the relaunch.
      settings = await openSettings(page, app);
      await automatic.click();
      await expect(automatic).toBeChecked();
      await radio(interval, "5 min").click();
      await radio(backups, "20").click();
      await automatic.click();
      await expect(automatic).not.toBeChecked();
      await closeSettings(settings);

      const closed = app.waitForEvent("close");
      await chooseMenu(page, app, "File", "Exit");
      await closed;
      await first.close();

      // Relaunch on the same profile: the choice is still there, and it applies to the engine.
      const second = await launchFlowApp(work, home);
      try {
        const page2 = second.page;
        await expect(page2).toHaveTitle("Untitled — Cutweave");
        const again = await openSettings(page2, second.app);
        await expect(again.getByRole("switch", { name: "Save automatically" })).not.toBeChecked();
        await expect(
          radio(again.getByRole("radiogroup", { name: "Save at least every" }), "5 min"),
        ).toHaveAttribute("aria-checked", "true");
        await expect(
          radio(again.getByRole("radiogroup", { name: "Backups to keep" }), "20"),
        ).toHaveAttribute("aria-checked", "true");
        await closeSettings(again);

        await openRecent(page2, "Autosave.cutweave");
        await expect(page2).toHaveTitle("Autosave — Cutweave");
        await expect(saveStatus(page2)).toHaveText("All changes saved");
        await importWithButton(second.app, page2, [work.file("broll-city.mp4")]);
        await expect(saveStatus(page2)).toHaveText("Unsaved changes, autosave is off");
        await staysUnsaved(page2);
        expect(mediaOnDisk(doc)).toBe(3);
      } finally {
        // Closing discards the unsaved edit (the journal keeps it for the next open).
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
