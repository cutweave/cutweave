// SPDX-License-Identifier: GPL-3.0-or-later
// F58 Media offline: the slate and the export (docs/user-flows.md, WP-2F.03c): a saved project
// with two clips; one media file is renamed away while the app is closed. Reopened, the preview
// shows the "media offline" slate where that clip plays (and the other clip's own picture next to
// it), and playback runs across it. File > Export… says which file is missing and offers
// Relink… and Export anyway: Export anyway writes a file (the toast says frames have offline
// media); Relink… opens the relink dialog, one folder search finds the renamed file, the preview
// shows the clip's picture again and the next export goes through with nothing to say.
//
// Needs an engine built with FFmpeg and the fetched corpus (`just corpus`), like F14; without
// either the flow skips and says why. Only the OS pickers are stubbed.
import { existsSync, renameSync, statSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import { copyCorpusClips, corpusDir, ffprobePath, probe } from "./export-helpers.ts";
import {
  chooseMenu,
  clickClip,
  clipAt,
  createProject,
  dirtyDot,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  inspector,
  launchFlowApp,
  layout,
  mediaCards,
  openRecent,
  pickFiles,
  profileDir,
  removeDir,
  rulerY,
  sec,
  shortcut,
  snapshot,
  timecode,
  timelineGeometry,
  toast,
  workspace,
  xAt,
} from "./helpers.ts";

/** The clip whose file goes missing (1080p, so the slate fills the frame) and the one that stays. */
const GONE = "h264_1080p60.mp4";
const KEPT = "sintel_trailer_480p.mp4";
const RENAMED = "street take 2.mp4";

/** The slate's red bands (`cutweave_render::slate::BAND`): two fifths of its picture. */
const SLATE_RED = [150, 28, 28] as const;

/** `HH:MM:SS:FF` → frames at 30 fps (the new project's rate). */
function frames(text: string | null): number {
  const [hh, mm, ss, ff] = (text ?? "").split(/[:;]/).map(Number);
  return (((hh ?? 0) * 60 + (mm ?? 0)) * 60 + (ss ?? 0)) * 30 + (ff ?? 0);
}

/**
 * The share of the preview a person sees in the slate's red: a screenshot of its canvas, decoded
 * and counted in the page (the test has no PNG decoder).
 */
async function slateShare(page: Page): Promise<number> {
  const box = await page.locator("canvas.cw-preview__canvas").boundingBox();
  if (box === null) return 0;
  const png = await page.screenshot({
    clip: {
      x: Math.round(box.x),
      y: Math.round(box.y),
      width: Math.max(1, Math.round(box.width)),
      height: Math.max(1, Math.round(box.height)),
    },
    animations: "allow",
    caret: "initial",
  });
  return page.evaluate(
    async ({ base64, rgb }) => {
      const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
      const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
      const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
      const ctx = canvas.getContext("2d");
      if (ctx === null) throw new Error("no 2D context");
      ctx.drawImage(bitmap, 0, 0);
      const { data } = ctx.getImageData(0, 0, bitmap.width, bitmap.height);
      let n = 0;
      for (let i = 0; i < data.length; i += 4) {
        const d =
          Math.abs((data[i] ?? 0) - rgb[0]) +
          Math.abs((data[i + 1] ?? 0) - rgb[1]) +
          Math.abs((data[i + 2] ?? 0) - rgb[2]);
        if (d < 48) n += 1;
      }
      return n / (data.length / 4);
    },
    { base64: png.toString("base64"), rgb: SLATE_RED },
  );
}

/** Puts the playhead at `seconds` by clicking the ruler. */
async function seek(page: Page, seconds: number, text: string): Promise<void> {
  const g = await timelineGeometry(page);
  await page.mouse.click(xAt(g, sec(seconds)), rulerY(g));
  await expect(timecode(page)).toHaveText(text);
}

test.describe("F58 media offline: slate and export", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("an offline clip shows the slate; Export offers Relink… and Export anyway", async () => {
    test.setTimeout(480_000);
    const info = test.info();
    const corpus = corpusDir();
    test.skip(corpus === null, "the corpus is not fetched (just corpus)");
    const ffprobe = ffprobePath();
    const work = workspace();
    const home = profileDir();
    const footage = path.join(work.dir, "footage");
    try {
      // A saved project: GONE at 0-2 s, KEPT at 2-4 s.
      const first = await launchFlowApp(work, home, { realMedia: true });
      {
        const { app, page } = first;
        await createProject(app, page, "Slate", work.dir);
        const files = copyCorpusClips(corpus ?? "", footage, [GONE, KEPT]);
        await pickFiles(app, files);
        await page.getByRole("button", { name: "Import", exact: true }).click();
        const refused = page.getByText(/not imported/).first();
        await expect(mediaCards(page).first().or(refused)).toBeVisible();
        test.skip(
          await refused.isVisible(),
          "this engine cannot read media; build it with FFmpeg (just e2e-flows does when the bundle is set up)",
        );
        await expect(mediaCards(page)).toHaveCount(2);
        const clips = async () => (await layout(page))[0]?.clips ?? [];
        await dropCard(page, GONE, "V1", 0);
        await expect.poll(async () => (await clips()).length).toBe(1);
        await editorCaughtUp(page);
        let g = await timelineGeometry(page);
        await clickClip(page, clipAt(g, "V1", 0).id);
        await inspector(page).duration.fill("2s");
        await page.keyboard.press("Enter");
        await expect.poll(async () => (await clips())[0]?.[1]).toBe(2);
        await editorCaughtUp(page);
        await dropCard(page, KEPT, "V1", 2);
        await expect.poll(async () => (await clips()).length).toBe(2);
        await editorCaughtUp(page);
        g = await timelineGeometry(page);
        await clickClip(page, clipAt(g, "V1", 2).id);
        await inspector(page).duration.fill("2s");
        await page.keyboard.press("Enter");
        await expect.poll(clips).toEqual([
          [0, 2],
          [2, 2],
        ]);
        await editorCaughtUp(page);
        await shortcut(app, "Control+S");
        await expect(dirtyDot(page)).toBeHidden();
        const closed = app.waitForEvent("close");
        await chooseMenu(page, app, "File", "Exit");
        await closed;
        await first.close();
      }

      // While the app is closed, one file gets another name.
      renameSync(path.join(footage, GONE), path.join(footage, RENAMED));

      const second = await launchFlowApp(work, home, { realMedia: true });
      try {
        const { app, page } = second;
        await openRecent(page, "Slate.cutweave");
        await expect(page).toHaveTitle("Slate — Cutweave");
        const relink = page.getByRole("dialog", { name: "1 media file is missing" });
        await expect(relink).toBeVisible();
        await relink.getByRole("button", { name: "Not now" }).click();
        await expect(relink).toBeHidden();

        // The preview: the slate where the offline clip plays, the other clip's own picture.
        const surface = page.locator(".cw-preview__surface");
        await expect(surface).toHaveAttribute("data-path", /^(gpu|jpeg)$/, { timeout: 30_000 });
        await seek(page, 1, "00:00:01:00");
        await expect.poll(() => slateShare(page), { timeout: 30_000 }).toBeGreaterThan(0.2);
        await snapshot(page, info, "slate in the preview");
        await seek(page, 3, "00:00:03:00");
        await expect.poll(() => slateShare(page), { timeout: 30_000 }).toBeLessThan(0.05);

        // Playback runs across the offline clip into the next one, and stops when asked.
        await seek(page, 0.5, "00:00:00:15");
        await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
        await page.keyboard.press("Space");
        await expect(page.getByRole("button", { name: "Pause", exact: true })).toBeVisible();
        await expect
          .poll(async () => frames(await timecode(page).textContent()), { timeout: 30_000 })
          .toBeGreaterThan(70);
        await page.keyboard.press("Space");
        await expect(page.getByRole("button", { name: "Play", exact: true })).toBeVisible();

        // File > Export…: the missing file is named, with the two ways on.
        const outDir = path.join(work.dir, "Slate");
        const withSlate = path.join(outDir, "with-slate.mp4");
        const relinked = path.join(outDir, "relinked.mp4");
        const dialog = page.getByRole("dialog", { name: "Export video" });
        const exportNow = dialog.getByRole("button", { name: "Export", exact: true });
        const notice = dialog.getByTestId("export-offline");
        await chooseMenu(page, app, "File", "Export…");
        await expect(dialog).toBeVisible();
        await dialog.getByLabel("Save to").fill(withSlate);
        await exportNow.click();
        await expect(notice.or(dialog.getByRole("alert")).first()).toBeVisible();
        await expect(notice).toContainText(`${GONE} is missing.`);
        await expect(notice).toContainText('"Media offline" card');
        await expect(notice.getByRole("button", { name: "Relink…" })).toBeVisible();
        await snapshot(page, info, "export refused");
        expect(existsSync(withSlate)).toBe(false);

        // Export anyway: the file is written, and the toast says some frames are offline.
        await notice.getByRole("button", { name: "Export anyway" }).click();
        const done = toast(page, "Export complete");
        await expect(done).toBeVisible({ timeout: 180_000 });
        await expect(done).toContainText("with-slate.mp4");
        await expect(done).toContainText("offline");
        expect(statSync(withSlate).size).toBeGreaterThan(10_000);
        if (ffprobe !== null) {
          const probed = probe(ffprobe, withSlate);
          expect(probed.video).toMatchObject({ codec: "h264", width: 1920, height: 1080 });
          expect(Math.abs(probed.duration - 4)).toBeLessThanOrEqual(1 / 30);
        }
        await expect(done).toBeHidden({ timeout: 30_000 });

        // Again, and this time Relink…: the relink dialog takes over, one search finds the file.
        await shortcut(app, "Control+M");
        await expect(dialog).toBeVisible();
        await dialog.getByLabel("Save to").fill(relinked);
        await exportNow.click();
        await expect(notice).toBeVisible();
        await notice.getByRole("button", { name: "Relink…" }).click();
        await expect(dialog).toBeHidden();
        await expect(relink).toBeVisible();
        await pickFiles(app, [footage]);
        await relink.getByRole("button", { name: "Search a folder…" }).click();
        const found = page.getByRole("dialog", { name: "All media found" });
        await expect(found).toBeVisible();
        await expect(found.getByRole("status")).toHaveText("Found 1 file.");
        await found.getByRole("button", { name: "Done" }).click();
        await expect(found).toBeHidden();
        await editorCaughtUp(page);

        // The clip's own picture is back in the preview.
        await seek(page, 1, "00:00:01:00");
        await expect.poll(() => slateShare(page), { timeout: 30_000 }).toBeLessThan(0.05);
        await snapshot(page, info, "relinked");

        // The export goes through with nothing to say about missing media.
        await shortcut(app, "Control+M");
        await expect(dialog).toBeVisible();
        await dialog.getByLabel("Save to").fill(relinked);
        await exportNow.click();
        const again = toast(page, "Export complete");
        await expect(again).toBeVisible({ timeout: 180_000 });
        await expect(again).toContainText("relinked.mp4");
        await expect(again).not.toContainText("offline");
        expect(statSync(relinked).size).toBeGreaterThan(10_000);
      } finally {
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
