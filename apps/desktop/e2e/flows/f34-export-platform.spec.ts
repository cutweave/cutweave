// SPDX-License-Identifier: GPL-3.0-or-later
// F34 Export for a platform (docs/user-flows.md, WP-2G.01, WP-2G.04): make the sequence 9:16 in the
// Inspector, put a clip and a still on it and stretch the still past Instagram Reels' 3 minutes;
// File > Export…: the portrait platform presets come first; Instagram Reels shows 1080 × 1920 and
// the platform's limit as a warning (not a block); export the first 4 s (a custom range) and
// check the file with ffprobe; then the queue: pause it in the Exports panel, choose YouTube 1080p
// (it says it fits with bars), add two exports to the queue, cancel one, resume, and the other is
// written at the sequence's shape. Needs the FFmpeg engine and the corpus (`just corpus`); without
// them the flow skips and says why. Only the OS file picker is stubbed.
import path from "node:path";
import type { Locator, Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import { copyCorpusClips, corpusDir, ffprobePath, probe } from "./export-helpers.ts";
import {
  chooseMenu,
  clickClip,
  clipAt,
  createProject,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  inspector,
  launchFlowApp,
  layout,
  mediaCards,
  pickFiles,
  sequenceFormat,
  shortcut,
  snapshot,
  timelineCanvas,
  timelineGeometry,
  toast,
  viewSettled,
  workspace,
} from "./helpers.ts";

const CLIPS = ["h264_1080p60.mp4", "image_png.png"] as const;
const BOUND = { timeout: 15_000 };

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

/** Types a custom range into the open export dialog. */
async function customRange(dialog: Locator, start: string, end: string): Promise<void> {
  await dialog.getByRole("radio", { name: "Custom" }).click();
  await dialog.getByLabel("Start", { exact: true }).fill(start);
  await dialog.getByLabel("End", { exact: true }).fill(end);
}

test.describe("F34 export for a platform", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("portrait presets first, the Reels limit as a warning, a 1080 × 1920 file, the queue", async () => {
    test.setTimeout(300_000);
    const info = test.info();
    const corpus = corpusDir();
    test.skip(corpus === null, "the corpus is not fetched (just corpus)");
    const ffprobe = ffprobePath();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work, undefined, { realMedia: true });
    try {
      const dialog = page.getByRole("dialog", { name: "Export video" });
      await createProject(app, page, "Reels", work.dir);

      // Nothing selected: the Inspector's Sequence section makes the canvas 1080 × 1920.
      const sequencePanel = page.getByTestId("sequence-inspector");
      await expect(sequencePanel).toBeVisible(BOUND);
      for (const [name, value] of [
        ["Width", "1080"],
        ["Height", "1920"],
      ] as const) {
        await sequencePanel.getByRole("spinbutton", { name }).dblclick();
        await page.keyboard.type(value);
        await page.keyboard.press("Enter");
      }
      await expect
        .poll(async () => {
          const f = await sequenceFormat(page);
          return [f.width, f.height];
        }, BOUND)
        .toEqual([1080, 1920]);
      await editorCaughtUp(page);

      // A clip with sound, then a still stretched to 200 s: 3:28 in all.
      const files = copyCorpusClips(corpus ?? "", work.media, CLIPS);
      await pickFiles(app, files);
      await page.getByRole("button", { name: "Import", exact: true }).click();
      const refused = page.getByText(/not imported/).first();
      await expect(mediaCards(page).first().or(refused)).toBeVisible();
      test.skip(
        await refused.isVisible(),
        "this engine cannot read media; build it with FFmpeg and the real media wiring (WP-1I.05)",
      );
      await expect(mediaCards(page)).toHaveCount(CLIPS.length);
      await timelineCanvas(page).locator("canvas").focus();
      await page.keyboard.press("-");
      await page.keyboard.press("-");
      // The drops and the click below are aimed with the zoom the engine reports: wait for it.
      // (Before the main track was magnetic a drop aimed with the old zoom landed late and the
      // flow followed it; on the magnetic track the still butts at the cut wherever it is
      // dropped, and only the click was off.)
      await viewSettled(page);
      const clips = async () => (await layout(page))[0]?.clips ?? [];
      await dropCard(page, "h264_1080p60.mp4", "V1", 0);
      await expect.poll(async () => (await clips()).length, BOUND).toBe(1);
      const video = (await clips())[0]?.[1] ?? 0;
      await editorCaughtUp(page);
      await dropCard(page, "image_png.png", "V1", video);
      await expect.poll(async () => (await clips()).length, BOUND).toBe(2);
      await editorCaughtUp(page);
      const still = (await clips())[1]?.[0] ?? video;
      await clickClip(page, clipAt(await timelineGeometry(page), "V1", still).id);
      await inspector(page).duration.fill("200s");
      await page.keyboard.press("Enter");
      await expect.poll(async () => (await clips())[1]?.[1], BOUND).toBe(200);
      await editorCaughtUp(page);
      const total = Math.round(still + 200);
      const clock = `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;

      // File > Export…: the presets made for 9:16 come first.
      await chooseMenu(page, app, "File", "Export…");
      await expect(dialog).toBeVisible();
      const preset = dialog.getByLabel("Preset", { exact: true });
      const first = preset.locator("optgroup").first();
      await expect(first).toHaveAttribute("label", "For this 9:16 sequence");
      await expect(first.locator("option")).toHaveText([
        "YouTube Shorts",
        "Instagram Reels",
        "Instagram Stories",
        "TikTok",
        "X portrait",
      ]);

      // Instagram Reels: 1080 × 1920, and the timeline is longer than Reels takes.
      await preset.selectOption({ label: "Instagram Reels" });
      const summary = dialog.getByTestId("export-summary");
      await expect(summary).toContainText("1080 × 1920 · 30 fps · H.264 + AAC · MP4", BOUND);
      const limit = dialog.getByText(
        `Instagram Reels takes videos up to 3:00; this export is ${clock}. You can still export it.`,
      );
      await expect(limit).toBeVisible(BOUND);
      await expect(dialog.getByRole("button", { name: "Export", exact: true })).toBeEnabled();
      await snapshot(page, info, "reels preset");

      // Export the first 4 s: the warning goes, the length follows.
      await customRange(dialog, "00:00:00:00", "00:00:04:00");
      await expect(summary).toContainText("Length 00:00:04:00", BOUND);
      await expect(limit).toBeHidden();
      const reels = path.join(work.dir, "Reels", "Reels.mp4");
      const norm = (p: string) => p.replace(/\\/g, "/").toLowerCase();
      expect(norm(await dialog.getByLabel("Save to").inputValue())).toBe(norm(reels));
      await dialog.getByRole("button", { name: "Export", exact: true }).click();
      const done = toast(page, "Export complete");
      await expect(done).toBeVisible({ timeout: 120_000 });
      await expect(done).toContainText("Reels.mp4");
      if (ffprobe === null) {
        info.annotations.push({ type: "note", description: "ffprobe is not on this machine" });
      } else {
        const probed = probe(ffprobe, reels);
        expect(probed.video).toMatchObject({ codec: "h264", width: 1080, height: 1920 });
        expect(probed.audio?.codec).toBe("aac");
        expect(Math.abs(probed.duration - 4)).toBeLessThanOrEqual(1 / 30);
        info.annotations.push({ type: "ffprobe", description: JSON.stringify(probed) });
      }

      // The queue: pause it, then add two exports with YouTube 1080p, which fits with bars.
      const panel = await openExportsPanel(page);
      await expect(panel.getByRole("listitem", { name: "Reels.mp4, Done" })).toBeVisible(BOUND);
      await panel.getByRole("button", { name: "Pause the queue" }).click();
      await expect(panel.getByRole("button", { name: "Resume the queue" })).toBeVisible(BOUND);
      const queued: string[] = [];
      for (const [name, start, end] of [
        ["wide-a.mp4", "00:00:00:00", "00:00:02:00"],
        ["wide-b.mp4", "00:00:02:00", "00:00:04:00"],
      ] as const) {
        await shortcut(app, "Control+M");
        await expect(dialog).toBeVisible();
        // The dialog starts at the preset used last.
        await expect(preset).toHaveValue(queued.length === 0 ? "instagram-reels" : "youtube-1080p");
        await preset.selectOption({ label: "YouTube 1080p" });
        await expect(
          dialog.getByText(
            /^Fits with bars: YouTube 1080p is 16:9 and this sequence is 9:16\. The file keeps the sequence's shape at 608 × 1080/,
          ),
        ).toBeVisible(BOUND);
        if (queued.length === 0) await snapshot(page, info, "fits with bars");
        await customRange(dialog, start, end);
        const file = path.join(work.dir, "Reels", name);
        await dialog.getByLabel("Save to").fill(file);
        await dialog.getByRole("button", { name: "Add to queue" }).click();
        await expect(dialog).toBeHidden();
        await expect(panel.getByRole("listitem", { name: `${name}, Waiting` })).toBeVisible(BOUND);
        queued.push(file);
      }
      // Cancel the second with its row's button; resume; the first is written.
      const second = panel.getByRole("listitem", { name: "wide-b.mp4, Waiting" });
      await second.getByRole("button", { name: "Cancel export" }).click();
      await expect(panel.getByRole("listitem", { name: "wide-b.mp4, Cancelled" })).toBeVisible(
        BOUND,
      );
      await snapshot(page, info, "export queue");
      await panel.getByRole("button", { name: "Resume the queue" }).click();
      await expect(panel.getByRole("listitem", { name: "wide-a.mp4, Done" })).toBeVisible({
        timeout: 120_000,
      });
      await expect(panel.getByRole("listitem", { name: "wide-b.mp4, Cancelled" })).toBeVisible();
      if (ffprobe !== null) {
        const wide = probe(ffprobe, queued[0] ?? "");
        expect(wide.video).toMatchObject({ codec: "h264", width: 608, height: 1080 });
        expect(Math.abs(wide.duration - 2)).toBeLessThanOrEqual(1 / 30);
      }
    } finally {
      await close();
      work.remove();
    }
  });
});
