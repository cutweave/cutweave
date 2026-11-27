// SPDX-License-Identifier: GPL-3.0-or-later
// F65 Your own export preset, Open file and the finished-export notification (docs/user-flows.md,
// WP-2G.04b): put a clip on the timeline; File > Export…: a MOV preset makes the save dialog offer
// MOV; change the size and quality under Advanced and save them as a preset under a name (the
// picker lists it under "Your presets" and chooses it); export with it and minimize the window:
// the OS notification says the export is complete; back in the app, the Exports panel opens the
// file from its row and from the row's menu, and the file has the preset's size; with
// Settings > Notifications > Finished exports off, the next export is not announced; delete the
// preset in the dialog and bring it back with the toast's Undo.
// Needs the FFmpeg engine and the corpus (`just corpus`); without them the flow skips and says
// why. Stubbed, in main, at the edge of the OS only: the save dialog, the shell's "open with the
// default app" and the notification the OS would draw.
import path from "node:path";
import type { ElectronApplication, Locator, Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import { copyCorpusClips, corpusDir, ffprobePath, probe } from "./export-helpers.ts";
import {
  chooseMenu,
  closeSettings,
  createProject,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  launchFlowApp,
  layout,
  mediaCards,
  openSettingsPage,
  pickFiles,
  shortcut,
  snapshot,
  timelineCanvas,
  toast,
  workspace,
} from "./helpers.ts";

const CLIP = "h264_1080p60.mp4";
const BOUND = { timeout: 15_000 };
const EXPORT_BOUND = { timeout: 120_000 };

interface SaveAsk {
  defaultPath: string | undefined;
  filters: { name: string; extensions: string[] }[];
}

declare global {
  /** What the stubbed OS edges of this flow were asked to do (installed by `stubOs`). */
  var __cutweaveFlowF65:
    | { opened: string[]; notices: { title: string; body: string }[]; saves: SaveAsk[] }
    | undefined;
}

/**
 * The OS is not ours: the save dialog answers with the path it was offered, `shell.openPath`
 * records the file instead of starting a player, and a notification is recorded instead of drawn
 * (the app still decides whether to show one).
 */
async function stubOs(app: ElectronApplication) {
  await app.evaluate(({ dialog, shell, Notification }) => {
    const seen = { opened: [], notices: [], saves: [] } as NonNullable<
      typeof globalThis.__cutweaveFlowF65
    >;
    globalThis.__cutweaveFlowF65 = seen;
    dialog.showSaveDialog = (async (...args: unknown[]) => {
      const options = (args.length > 1 ? args[1] : args[0]) as SaveAsk;
      seen.saves.push({ defaultPath: options.defaultPath, filters: options.filters });
      return { canceled: false, filePath: options.defaultPath ?? "" };
    }) as typeof dialog.showSaveDialog;
    shell.openPath = async (target: string) => {
      seen.opened.push(target);
      return "";
    };
    Notification.isSupported = () => true;
    Notification.prototype.show = function show(this: { title: string; body: string }) {
      seen.notices.push({ title: this.title, body: this.body });
    };
  });
  const read = () =>
    app.evaluate(() => globalThis.__cutweaveFlowF65 ?? { opened: [], notices: [], saves: [] });
  return {
    opened: async () => (await read()).opened,
    notices: async () => (await read()).notices,
    saves: async () => (await read()).saves,
  };
}

/** The person minimizes the editor window (the OS's own button), or brings it back. */
async function minimize(app: ElectronApplication, minimized: boolean): Promise<void> {
  await app.evaluate(({ BrowserWindow }, minimized) => {
    const win = BrowserWindow.getAllWindows().find(
      (w) => !w.webContents.getURL().endsWith("/popout.html"),
    );
    if (win === undefined) throw new Error("no editor window");
    if (minimized) {
      win.minimize();
    } else {
      win.restore();
      win.focus();
    }
  }, minimized);
  await expect
    .poll(() =>
      app.evaluate(({ BrowserWindow }) =>
        BrowserWindow.getAllWindows().some(
          (w) => !w.webContents.getURL().endsWith("/popout.html") && w.isMinimized(),
        ),
      ),
    )
    .toBe(minimized);
}

/** View > Panels > Exports, with real clicks. */
async function openExportsPanel(page: Page): Promise<Locator> {
  await page
    .getByRole("menubar", { name: "Application menu" })
    .getByRole("menuitem", { name: "View", exact: true })
    .click();
  await page.getByRole("menu", { name: "View" }).getByRole("menuitem", { name: "Panels" }).click();
  await page
    .getByRole("menu", { name: "Panels" })
    .getByRole("menuitemcheckbox", { name: "Exports", exact: true })
    .click();
  const panel = page.getByTestId("export-queue");
  await expect(panel).toBeVisible();
  return panel;
}

const norm = (p: string) => p.replace(/\\/g, "/").toLowerCase();

test.describe("F65 your own export preset, Open file and the notification", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("save the dialog's settings as a preset, export in the background, open the file", async () => {
    test.setTimeout(420_000);
    const info = test.info();
    const corpus = corpusDir();
    test.skip(corpus === null, "the corpus is not fetched (just corpus)");
    const ffprobe = ffprobePath();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work, undefined, { realMedia: true });
    try {
      const dialog = page.getByRole("dialog", { name: "Export video" });
      const preset = dialog.getByLabel("Preset", { exact: true });
      await createProject(app, page, "Own", work.dir);
      const os = await stubOs(app);

      // One clip on the timeline.
      const files = copyCorpusClips(corpus ?? "", work.media, [CLIP]);
      await pickFiles(app, files);
      await page.getByRole("button", { name: "Import", exact: true }).click();
      const refused = page.getByText(/not imported/).first();
      await expect(mediaCards(page).first().or(refused)).toBeVisible();
      test.skip(
        await refused.isVisible(),
        "this engine cannot read media; build it with FFmpeg and the real media wiring (WP-1I.05)",
      );
      await timelineCanvas(page).locator("canvas").focus();
      await dropCard(page, CLIP, "V1", 0);
      await expect.poll(async () => ((await layout(page))[0]?.clips ?? []).length, BOUND).toBe(1);
      await editorCaughtUp(page);

      // File > Export…: a preset that writes MOV makes the save dialog offer MOV.
      await chooseMenu(page, app, "File", "Export…");
      await expect(dialog).toBeVisible();
      await preset.selectOption({ label: "ProRes 422 HQ" });
      await expect(dialog.getByLabel("Save to")).toHaveValue(/\.mov$/);
      await dialog.getByRole("button", { name: "Choose…" }).click();
      await expect.poll(async () => (await os.saves()).length, BOUND).toBe(1);
      const asked = (await os.saves())[0];
      expect(asked?.filters).toEqual([{ name: "QuickTime movie", extensions: ["mov"] }]);
      expect(norm(asked?.defaultPath ?? "")).toBe(norm(path.join(work.dir, "Own", "Own.mov")));
      await expect(dialog.getByLabel("Save to")).toHaveValue(/Own\.mov$/);

      // The general preset at 1280 × 720 and Low quality, saved as "Review copy".
      await preset.selectOption("h264-mp4-1080p");
      await dialog.getByRole("button", { name: "Advanced" }).click();
      await dialog.getByLabel("Width", { exact: true }).fill("1280");
      await dialog.getByLabel("Height", { exact: true }).fill("720");
      await dialog.getByRole("radio", { name: "Low" }).click();
      await dialog.getByRole("button", { name: "Save as preset…" }).click();
      const name = dialog.getByLabel("Preset name");
      await expect(name).toBeFocused();
      // Enter with no name asks for one; it does not start the export.
      await page.keyboard.press("Enter");
      await expect(dialog.getByText("Type a name for the preset.")).toBeVisible();
      const box = await dialog.boundingBox();
      const view = await page.evaluate(() => window.innerHeight);
      expect(box?.height ?? 0).toBeLessThanOrEqual(view * 0.6 + 1);
      await snapshot(page, info, "save as preset");
      await page.keyboard.type("Review copy");
      await page.keyboard.press("Enter");
      await expect(toast(page, "Preset saved")).toContainText(
        "Review copy is under Your presets.",
        BOUND,
      );
      await expect(name).toBeHidden();
      await expect(preset).toHaveValue("review-copy");
      await expect(preset.locator('optgroup[label="Your presets"] option')).toHaveText([
        "Review copy",
      ]);
      await expect(dialog.getByTestId("export-summary")).toContainText("1280 × 720", BOUND);
      await expect(dialog.getByRole("button", { name: "Delete this preset" })).toBeVisible();
      await snapshot(page, info, "own preset chosen");

      // Export with it and minimize the window: the OS notification says it is complete.
      const first = path.join(work.dir, "Own", "review.mp4");
      await dialog.getByLabel("Save to").fill(first);
      await dialog.getByRole("button", { name: "Export", exact: true }).click();
      await minimize(app, true);
      await expect
        .poll(os.notices, EXPORT_BOUND)
        .toEqual([{ title: "Export complete", body: "review.mp4" }]);
      await minimize(app, false);

      // The Exports panel: open the file from its row's button and from the row's menu.
      const panel = await openExportsPanel(page);
      const row = panel.getByRole("listitem", { name: "review.mp4, Done" });
      await expect(row).toBeVisible(BOUND);
      await row.getByRole("button", { name: "Open file" }).click();
      await expect.poll(async () => (await os.opened()).map(norm), BOUND).toEqual([norm(first)]);
      await row.click({ button: "right" });
      await page
        .getByRole("menu", { name: "Export actions for review.mp4" })
        .getByRole("menuitem", { name: "Open file" })
        .click();
      await expect
        .poll(async () => (await os.opened()).map(norm), BOUND)
        .toEqual([norm(first), norm(first)]);
      await snapshot(page, info, "open file");
      if (ffprobe === null) {
        info.annotations.push({ type: "note", description: "ffprobe is not on this machine" });
      } else {
        const probed = probe(ffprobe, first);
        expect(probed.video).toMatchObject({ codec: "h264", width: 1280, height: 720 });
        info.annotations.push({ type: "ffprobe", description: JSON.stringify(probed) });
      }

      // Settings > Notifications: the two export rows; with Finished exports off, the next
      // export is not announced.
      const settings = await openSettingsPage(page, app, "Notifications");
      const announce = settings.getByRole("switch", { name: "Finished exports" });
      await expect(announce).toHaveAttribute("aria-checked", "true");
      await expect(settings.getByRole("switch", { name: "Export sound" })).toHaveAttribute(
        "aria-checked",
        "false",
      );
      await announce.click();
      await expect(announce).toHaveAttribute("aria-checked", "false");
      await snapshot(page, info, "notification settings");
      await closeSettings(settings);
      await shortcut(app, "Control+M");
      await expect(dialog).toBeVisible();
      // The dialog starts at the preset used last: the person's own.
      await expect(preset).toHaveValue("review-copy");
      const second = path.join(work.dir, "Own", "quiet.mp4");
      await dialog.getByLabel("Save to").fill(second);
      await dialog.getByRole("button", { name: "Export", exact: true }).click();
      await minimize(app, true);
      // The panel's rows are read while the window is minimized: no click, no paint needed.
      await expect
        .poll(() => panel.locator('li[data-state="succeeded"]').count(), EXPORT_BOUND)
        .toBe(2);
      await minimize(app, false);
      await expect(panel.getByRole("listitem", { name: "quiet.mp4, Done" })).toBeVisible(BOUND);
      expect(await os.notices()).toHaveLength(1);

      // Delete the preset in the dialog; Undo on the toast brings it back.
      await shortcut(app, "Control+M");
      await expect(dialog).toBeVisible();
      await expect(preset).toHaveValue("review-copy");
      await dialog.getByRole("button", { name: "Delete this preset" }).click();
      const deleted = toast(page, "Preset deleted");
      await expect(deleted).toContainText("Review copy", BOUND);
      await expect(preset).toHaveValue("h264-mp4-1080p");
      await expect(preset.locator('optgroup[label="Your presets"]')).toHaveCount(0);
      await expect(dialog.getByRole("button", { name: "Delete this preset" })).toBeHidden();
      await deleted.getByRole("button", { name: "Undo" }).click();
      await expect(preset.locator('optgroup[label="Your presets"] option')).toHaveText(
        ["Review copy"],
        BOUND,
      );
    } finally {
      await close();
      work.remove();
    }
  });
});
