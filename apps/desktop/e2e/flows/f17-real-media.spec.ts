// SPDX-License-Identifier: GPL-3.0-or-later
// F17 Real media (docs/user-flows.md; WP-1I.05): F3-F5 with real decoding instead of the dev
// stub. Import H.264 1080p60, HEVC 10-bit and the rotated VFR phone clip from the corpus: the
// cards show what FFmpeg probed (codec, size, frame rate, duration) and the engine analyses each
// file in the background (frame index, thumbnails and filmstrip, waveform). Place two clips, play
// across the cut, and step frame by frame: every step shows its own picture, and stepping back
// shows the same pictures again (frame-accurate preview from the decoder pool).
//
// Not yet visible: the cards' thumbnails and the timeline's waveforms are generated into the
// cache here, but reach the UI through the `/media` HTTP routes of WP-1I.03 (the cards draw
// their placeholder tile until then). Needs an engine built with FFmpeg (`just e2e-flows`
// builds it when the bundle is set up) and the fetched corpus (`just corpus`); without either
// the flow skips and says why.
import { createHash } from "node:crypto";
import type { Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import { copyCorpusClips, corpusDir } from "./export-helpers.ts";
import {
  createProject,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  launchFlowApp,
  layout,
  mediaCard,
  mediaCards,
  pickFiles,
  rulerY,
  sec,
  snapshot,
  timecode,
  timelineGeometry,
  workspace,
  xAt,
} from "./helpers.ts";

const H264 = "h264_1080p60.mp4";
const HEVC = "hevc_10bit_sdr.mp4";
const PHONE = "vfr_phone_rotated90.mp4";

/** `HH:MM:SS:FF` → frames at 30 fps (the new project's rate). */
async function timecodeFrames(page: Page): Promise<number> {
  const [hh, mm, ss, ff] = ((await timecode(page).textContent()) ?? "").split(/[:;]/).map(Number);
  return (((hh ?? 0) * 60 + (mm ?? 0)) * 60 + (ss ?? 0)) * 30 + (ff ?? 0);
}

/**
 * A hash of the preview's upper half (what the person sees; see F13): the corpus test patterns
 * move there (a sweeping line, dots), and the overlay's timecode readout sits in the lower left.
 */
async function previewHash(page: Page): Promise<string> {
  const box = await page.locator("canvas.cw-preview__canvas").boundingBox();
  if (box === null) throw new Error("no preview canvas");
  const png = await page.screenshot({
    clip: {
      x: Math.round(box.x + 4),
      y: Math.round(box.y + 4),
      width: Math.round(box.width - 8),
      height: Math.round(box.height / 2 - 4),
    },
    animations: "allow",
    caret: "initial",
  });
  return createHash("sha1").update(png).digest("hex");
}

/** Resolves once `read` gives the same answer twice in a row. */
async function settled<T>(read: () => Promise<T>): Promise<T> {
  let last: T | undefined;
  let value: T | undefined;
  await expect
    .poll(
      async () => {
        value = await read();
        const same = value === last;
        last = value;
        return same;
      },
      { intervals: [150, 250, 400] },
    )
    .toBe(true);
  return value as T;
}

async function press(page: Page, key: string): Promise<void> {
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press(key);
}

interface JobView {
  readonly label: string;
  readonly state: string;
}

test.describe("F17 real media", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("real probe data, background analysis, playback and frame-accurate steps", async () => {
    test.setTimeout(300_000);
    const info = test.info();
    const corpus = corpusDir();
    test.skip(corpus === null, "the corpus is not fetched (just corpus)");
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work, undefined, { realMedia: true });
    try {
      const caps = await editorCall<{ available?: boolean }>(page, "media.capabilities");
      test.skip(
        caps.available !== true,
        "this engine has no media backend; build it with --features cutweave-engine/ffmpeg",
      );
      await createProject(app, page, "Real", work.dir);

      // Import: the cards show what FFmpeg found in each file.
      const files = copyCorpusClips(corpus ?? "", work.media, [H264, HEVC, PHONE]);
      await pickFiles(app, files);
      await page.getByRole("button", { name: "Import", exact: true }).click();
      await expect(mediaCards(page)).toHaveCount(3);
      await expect(mediaCard(page, H264)).toContainText("1920×1080");
      await expect(mediaCard(page, H264)).toContainText("60 fps");
      await expect(mediaCard(page, H264)).toContainText("H264");
      await expect(mediaCard(page, H264)).toContainText("0:08");
      await expect(mediaCard(page, HEVC)).toContainText("HEVC");
      await expect(mediaCard(page, HEVC)).toContainText("30 fps");
      await expect(mediaCard(page, HEVC)).toContainText("0:05");
      await expect(mediaCard(page, PHONE)).toContainText("H264");
      await expect(mediaCard(page, PHONE)).toContainText("0:10");
      await snapshot(page, info, "imported");

      // The engine indexed and thumbnailed every file in the background, and measured the
      // sound of those that have some (the HEVC clip is silent: no waveform job).
      const media = await editorCall<{ media: { id: string; path: string }[] }>(page, "media.list");
      expect(media.media).toHaveLength(3);
      for (const asset of media.media) {
        const expected = asset.path.endsWith(HEVC)
          ? "index:succeeded thumbnails:succeeded"
          : "index:succeeded thumbnails:succeeded waveform:succeeded";
        await expect
          .poll(
            async () => {
              const { jobs } = await editorCall<{ jobs: JobView[] }>(page, "job.list", {
                media_id: asset.id,
              });
              return jobs
                .map((j) => `${j.label}:${j.state}`)
                .sort()
                .join(" ");
            },
            { timeout: 120_000, message: `the analysis of ${asset.path}` },
          )
          .toBe(expected);
      }

      // Place the HEVC clip, then the phone clip right after it.
      await dropCard(page, HEVC, "V1", 0);
      await expect.poll(async () => (await layout(page))[0]?.clips.length ?? 0).toBe(1);
      await editorCaughtUp(page);
      const first = (await layout(page))[0]?.clips[0]?.[1] ?? 0;
      expect(first).toBeCloseTo(5, 1);
      await dropCard(page, PHONE, "V1", first);
      await expect.poll(async () => (await layout(page))[0]?.clips.length ?? 0).toBe(2);
      await editorCaughtUp(page);

      // The preview streams the real frames.
      const surface = page.locator(".cw-preview__surface");
      await expect(surface).toHaveAttribute("data-path", /^(gpu|jpeg)$/, { timeout: 20_000 });
      test.skip(
        (await surface.getAttribute("data-path")) !== "gpu",
        "the preview fell back to still frames; the JPEG path needs /media/frame (WP-1I.03)",
      );
      await expect(surface).toHaveAttribute("data-live", "true", { timeout: 30_000 });

      // Play across the cut from 4 s: the timecode runs past it and the picture changes.
      const g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(4)), rulerY(g));
      await expect(timecode(page)).toHaveText("00:00:04:00");
      const before = await settled(() => previewHash(page));
      await press(page, "Space");
      await expect(page.getByRole("button", { name: "Pause", exact: true })).toBeVisible();
      await expect
        .poll(() => timecodeFrames(page), { timeout: 30_000 })
        .toBeGreaterThan(Math.round(first * 30) + 15);
      await press(page, "Space");
      await expect(page.getByRole("button", { name: "Play", exact: true })).toBeVisible();
      await settled(() => timecodeFrames(page));
      expect(await settled(() => previewHash(page))).not.toBe(before);
      await snapshot(page, info, "phone clip");

      // Frame-accurate steps in both clips: the HEVC clip is 30 fps on a 30 fps timeline, so
      // every step is a new picture; stepping back shows the same pictures again.
      for (const at of [1, first + 1]) {
        await page.mouse.click(xAt(g, sec(at)), rulerY(g));
        const start = Math.round(at * 30);
        await expect.poll(() => timecodeFrames(page)).toBe(start);
        const hashes = [await settled(() => previewHash(page))];
        for (let i = 1; i <= 3; i += 1) {
          await press(page, "ArrowRight");
          await expect.poll(() => timecodeFrames(page)).toBe(start + i);
          hashes.push(await settled(() => previewHash(page)));
        }
        if (at === 1) expect(new Set(hashes).size).toBe(4);
        for (let i = 2; i >= 0; i -= 1) {
          await press(page, "ArrowLeft");
          await expect.poll(() => timecodeFrames(page)).toBe(start + i);
          await expect.poll(() => previewHash(page)).toBe(hashes[i]);
        }
      }
    } finally {
      await close();
      work.remove();
    }
  });
});
