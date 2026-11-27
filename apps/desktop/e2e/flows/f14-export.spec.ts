// SPDX-License-Identifier: GPL-3.0-or-later
// F14 Export (docs/user-flows.md, WP-1M.08; M1 exit 1): import three corpus clips, place, trim and
// split them, then File > Export… (menu, shortcut and the top bar's button): choose settings,
// cancel one export (no file is left), export a 1080p MP4 and check it with ffprobe and a
// Chromium `<video>`. Needs an engine built with FFmpeg (`--features cutweave-engine/ffmpeg`,
// WP-1I.05 makes `just e2e-flows` build it) and the fetched corpus (`just corpus`); without
// either the flow skips and says why. Only the OS pickers and "reveal in folder" are stubbed.
import { existsSync, statSync } from "node:fs";
import path from "node:path";
import { editorCall, expect, test } from "../fixtures.ts";
import {
  copyCorpusClips,
  corpusDir,
  ffprobePath,
  playInChromium,
  probe,
} from "./export-helpers.ts";
import {
  chooseMenu,
  clickClip,
  clipAt,
  createProject,
  drag,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  inspector,
  laneY,
  launchFlowApp,
  layout,
  mediaCards,
  pickFiles,
  pickSavePath,
  rulerY,
  sec,
  shortcut,
  snapshot,
  timecode,
  timelineCanvas,
  timelineGeometry,
  toast,
  workspace,
  xAt,
} from "./helpers.ts";

const CLIPS = ["h264_1080p60.mp4", "no_audio_stream.mp4", "sintel_trailer_480p.mp4"] as const;

test.describe("F14 export", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("the export dialog: settings, cancel, a 1080p MP4 that plays, replace prompt", async () => {
    test.setTimeout(300_000);
    const info = test.info();
    const corpus = corpusDir();
    test.skip(corpus === null, "the corpus is not fetched (just corpus)");
    const ffprobe = ffprobePath();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work, undefined, { realMedia: true });
    try {
      const exportButton = page.getByRole("button", { name: "Export", exact: true });
      const dialog = page.getByRole("dialog", { name: "Export video" });
      const progress = page.getByRole("dialog", { name: /^Exporting / });

      // An empty timeline has nothing to export: the button and the menu item are off.
      await createProject(app, page, "Cut", work.dir);
      await expect(exportButton).toBeDisabled();
      await shortcut(app, "Control+M");
      await expect(dialog).toBeHidden();

      // Import three corpus clips; place two, trim and split.
      const files = copyCorpusClips(corpus ?? "", work.media, CLIPS);
      // An engine without FFmpeg cannot read the clips: skip, saying what is missing.
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
      const zoom = async () =>
        (await editorCall<{ zoom: number | null }>(page, "view.get")).zoom ?? 100;
      await page.keyboard.press("-");
      await expect.poll(zoom).toBe(80);
      await page.keyboard.press("-");
      await expect.poll(zoom).toBeCloseTo(64, 6);
      const clips = async () => (await layout(page))[0]?.clips ?? [];
      await dropCard(page, "h264_1080p60.mp4", "V1", 0);
      await expect.poll(async () => (await clips()).length).toBe(1);
      const a = (await clips())[0]?.[1] ?? 0;
      expect(a).toBeGreaterThan(4);
      await editorCaughtUp(page);

      // Trim A's end by 2 s with the pointer.
      let g = await timelineGeometry(page);
      const end = xAt(g, sec(a));
      await drag(
        page,
        { x: end - 2, y: laneY(g, "V1") },
        { x: end - 2 - 2 * g.pxPerSec, y: laneY(g, "V1") },
      );
      await expect.poll(clips).toEqual([[0, a - 2]]);
      await editorCaughtUp(page);

      // B (no audio) after it, then C (a minute long) cut down to 4 s in the inspector.
      await dropCard(page, "no_audio_stream.mp4", "V1", a - 2);
      await expect.poll(async () => (await clips()).length).toBe(2);
      const b = (await clips())[1]?.[1] ?? 0;
      await editorCaughtUp(page);
      await dropCard(page, "sintel_trailer_480p.mp4", "V1", a - 2 + b);
      await expect.poll(async () => (await clips()).length).toBe(3);
      await editorCaughtUp(page);
      g = await timelineGeometry(page);
      await clickClip(page, clipAt(g, "V1", a - 2 + b).id);
      await inspector(page).duration.fill("4s");
      await page.keyboard.press("Enter");
      await expect.poll(async () => (await clips())[2]?.[1]).toBe(4);
      await editorCaughtUp(page);

      // Split A at 3 s: click the ruler, select A, press S.
      g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(3)), rulerY(g));
      await expect(timecode(page)).toHaveText("00:00:03:00");
      await clickClip(page, clipAt(g, "V1", 0).id);
      await page.keyboard.press("s");
      await expect.poll(clips).toEqual([
        [0, 3],
        [3, a - 5],
        [a - 2, b],
        [a - 2 + b, 4],
      ]);
      await editorCaughtUp(page);
      const timelineSeconds = a - 2 + b + 4;

      const outDir = path.join(work.dir, "Cut");
      const cancelled = path.join(outDir, "cut-cancelled.mp4");
      const finalFile = path.join(outDir, "cut-final.mp4");

      // The dialog: the project's folder and name, the general 1080p preset (WP-2G.04), the
      // sequence's size and rate under Advanced.
      await exportButton.click();
      await expect(dialog).toBeVisible();
      await expect(dialog.getByLabel("Preset")).toHaveValue("h264-mp4-1080p");
      await expect(dialog.getByTestId("export-summary")).toContainText(
        "1920 × 1080 · 30 fps · H.264 + AAC · MP4",
      );
      await snapshot(page, info, "export dialog");
      const norm = (p: string) => p.replace(/\\/g, "/").toLowerCase();
      expect(norm(await dialog.getByLabel("Save to").inputValue())).toBe(
        norm(path.join(outDir, "Cut.mp4")),
      );
      await dialog.getByRole("button", { name: "Advanced" }).click();
      await expect(dialog.getByLabel("Width")).toHaveValue("1920");
      await expect(dialog.getByLabel("Height")).toHaveValue("1080");
      await expect(dialog.getByLabel("Frame rate")).toHaveValue("30");
      await expect(dialog.getByRole("radio", { name: "Medium" })).toBeChecked();
      await expect(dialog.getByRole("radio", { name: "Whole sequence" })).toBeChecked();
      const hardware = dialog.getByRole("switch", { name: "Use hardware encoder" });
      await expect(hardware).toBeChecked();
      // Auto names what this computer has, or says it will choose.
      await expect(
        dialog.getByText(
          /Found .+\. Faster than software|No hardware encoder|Uses a hardware encoder/,
        ),
      ).toBeVisible();

      // Bad values are refused in the dialog, in words.
      await dialog.getByLabel("Width").fill("1921");
      await dialog.getByRole("button", { name: "Export", exact: true }).click();
      await expect(dialog.getByRole("alert")).toHaveText(
        "Width must be an even number from 16 to 8192.",
      );
      await dialog.getByLabel("Width").fill("3840");
      await dialog.getByLabel("Height").fill("2160");
      await dialog.getByRole("radio", { name: "Low" }).click();
      await hardware.click();
      await expect(hardware).not.toBeChecked();
      await expect(dialog.getByText("Software only: slower, works on any computer.")).toBeVisible();

      // Cancel one: a large software export, hidden (the chip carries on), brought back, cancelled.
      await dialog.getByLabel("Save to").fill(cancelled);
      await dialog.getByRole("button", { name: "Export", exact: true }).click();
      if (!(await progress.isVisible().catch(() => false))) {
        await expect(progress.or(dialog.getByRole("alert"))).toBeVisible();
        const refusal = await dialog
          .getByRole("alert")
          .textContent({ timeout: 1000 })
          .catch(() => null);
        if (refusal !== null) {
          test.skip(
            /unsupported|encoder|not built|ffmpeg/i.test(refusal),
            `this engine cannot export (${refusal}); build it with --features cutweave-engine/ffmpeg (WP-1I.05)`,
          );
          throw new Error(`the export did not start: ${refusal}`);
        }
      }
      await expect(progress).toBeVisible();
      await expect(progress.getByRole("progressbar", { name: "Export progress" })).toBeVisible();
      await progress.getByRole("button", { name: "Hide" }).click();
      await expect(progress).toBeHidden();
      const chip = page.getByTestId("export-chip");
      await expect(chip).toHaveText(/^Exporting \d+%$/);
      await chip.click();
      await expect(progress).toBeVisible();
      await snapshot(page, info, "export progress");
      await progress.getByRole("button", { name: "Cancel export" }).click();
      await expect(toast(page, "Export cancelled")).toBeVisible();
      await expect(progress).toBeHidden();
      await expect(chip).toBeHidden();
      expect(existsSync(cancelled)).toBe(false);

      // Export it: File > Export…, the native save dialog picks the file, the defaults as they are (Medium).
      const proposed = await pickSavePath(app, finalFile);
      await chooseMenu(page, app, "File", "Export…");
      await expect(dialog).toBeVisible();
      await expect(dialog.getByTestId("export-summary")).toContainText("1920 × 1080");
      await dialog.getByRole("button", { name: "Choose…" }).click();
      await expect(dialog.getByLabel("Save to")).toHaveValue(finalFile);
      expect((await proposed()).map((p) => norm(p ?? ""))).toEqual([
        norm(path.join(outDir, "Cut.mp4")),
      ]);
      await app.evaluate(({ shell }) => {
        const seen: string[] = [];
        (globalThis as unknown as { __revealed: string[] }).__revealed = seen;
        shell.showItemInFolder = (p: string) => {
          seen.push(p);
        };
      });
      await dialog.getByRole("button", { name: "Export", exact: true }).click();
      const done = toast(page, "Export complete");
      await expect(done).toBeVisible({ timeout: 120_000 });
      await expect(progress).toBeHidden();
      await expect(done).toContainText("cut-final.mp4");
      await done.getByRole("button", { name: "Reveal in folder" }).click();
      expect(
        await app.evaluate(() => (globalThis as unknown as { __revealed: string[] }).__revealed),
      ).toEqual([finalFile]);
      await snapshot(page, info, "export complete");

      // The file: ffprobe accepts it, it is the timeline's length, and Chromium plays it.
      expect(statSync(finalFile).size).toBeGreaterThan(10_000);
      if (ffprobe === null) {
        info.annotations.push({
          type: "note",
          description: "ffprobe is not on this machine (set FFMPEG_DIR); the file was not probed",
        });
      } else {
        const probed = probe(ffprobe, finalFile);
        expect(probed.formatName).toContain("mp4");
        expect(probed.video).toMatchObject({ codec: "h264", width: 1920, height: 1080 });
        expect(probed.video?.frameRate).toBeCloseTo(30, 3);
        expect(probed.audio?.codec).toBe("aac");
        expect(Math.abs(probed.duration - timelineSeconds)).toBeLessThanOrEqual(1 / 30);
        info.annotations.push({ type: "ffprobe", description: JSON.stringify(probed) });
      }
      const played = await playInChromium(finalFile);
      expect(played.error).toBeNull();
      expect(played.readyState).toBeGreaterThanOrEqual(3);
      expect(played.videoWidth).toBe(1920);
      expect(Math.abs(played.duration - timelineSeconds)).toBeLessThanOrEqual(1 / 30);
      info.annotations.push({ type: "video", description: JSON.stringify(played) });

      // The timeline was not touched by exporting it.
      expect(
        (await editorCall<{ sequences: unknown[] }>(page, "project.info")).sequences,
      ).toHaveLength(1);
      expect(await clips()).toHaveLength(4);

      // Exporting to a file that exists asks first; Back keeps the file and the settings.
      const before = statSync(finalFile).mtimeMs;
      await shortcut(app, "Control+M");
      await expect(dialog).toBeVisible();
      await dialog.getByLabel("Save to").fill(finalFile);
      await dialog.getByRole("button", { name: "Export", exact: true }).click();
      const replace = page.getByRole("dialog", { name: "Replace cut-final.mp4?" });
      await expect(replace).toBeVisible();
      await snapshot(page, info, "replace prompt");
      await replace.getByRole("button", { name: "Back" }).click();
      await expect(dialog.getByLabel("Save to")).toHaveValue(finalFile);
      await dialog.getByRole("button", { name: "Cancel" }).click();
      await expect(dialog).toBeHidden();
      expect(statSync(finalFile).mtimeMs).toBe(before);
    } finally {
      await close();
      work.remove();
    }
  });
});
