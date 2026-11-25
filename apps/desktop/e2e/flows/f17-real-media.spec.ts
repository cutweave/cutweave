// SPDX-License-Identifier: GPL-3.0-or-later
// F17 Real media (docs/user-flows.md; WP-1I.05, WP-1I.03): F3-F5 with real decoding instead of
// the dev stub. Import H.264 1080p60, HEVC 10-bit and the rotated VFR phone clip from the corpus:
// the cards show what FFmpeg probed (codec, size, frame rate, duration) and the engine analyses
// each file in the background (frame index, thumbnails and filmstrip, waveform). The cards then
// show real thumbnails, and once two clips are placed the video clip draws a filmstrip and the
// phone clip's sound (its linked audio clip) a waveform: all checked from the pixels on screen,
// against the flat placeholder tiles. Then play across the cut and step frame by frame: every
// step shows its own picture, and stepping back shows the same pictures again (frame-accurate
// preview from the decoder pool).
//
// Needs an engine built with FFmpeg (`just e2e-flows` builds it when the bundle is set up) and
// the fetched corpus (`just corpus`); without either the flow skips and says why.
import { createHash } from "node:crypto";
import type { Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import { copyCorpusClips, corpusDir } from "./export-helpers.ts";
import {
  clipBox,
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
  type TimelineGeometry,
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

interface PixelStats {
  /** Colours in the region, quantized to 4 bits a channel. */
  readonly colours: number;
  /** Share of the pixels that have the most common of them. */
  readonly dominant: number;
}

type Region = { x: number; y: number; width: number; height: number };

/**
 * What the person sees in `region` of the window: a screenshot, decoded and counted. The
 * placeholders (a card's icon tile, a clip body without tiles or peaks) are flat: one colour
 * covers nearly all of it. A video frame, a filmstrip or a waveform is not.
 */
async function pixels(page: Page, region: Region): Promise<PixelStats> {
  const png = await page.screenshot({
    clip: {
      x: Math.round(region.x),
      y: Math.round(region.y),
      width: Math.max(1, Math.round(region.width)),
      height: Math.max(1, Math.round(region.height)),
    },
    animations: "allow",
    caret: "initial",
  });
  // Decoded in the page (the test has no PNG decoder); only reads the screenshot's bytes.
  return page.evaluate(async (base64) => {
    const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
    const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
    const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
    const ctx = canvas.getContext("2d");
    if (ctx === null) throw new Error("no 2D context");
    ctx.drawImage(bitmap, 0, 0);
    const { data } = ctx.getImageData(0, 0, bitmap.width, bitmap.height);
    const counts = new Map<number, number>();
    for (let i = 0; i < data.length; i += 4) {
      const key =
        (((data[i] ?? 0) >> 4) << 8) | (((data[i + 1] ?? 0) >> 4) << 4) | ((data[i + 2] ?? 0) >> 4);
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    return { colours: counts.size, dominant: Math.max(...counts.values()) / (data.length / 4) };
  }, png.toString("base64"));
}

/** A card's picture area. */
async function cardThumb(page: Page, name: string): Promise<Region> {
  const box = await mediaCard(page, name).locator(".cw-media__thumb").boundingBox();
  if (box === null) throw new Error(`the card of ${name} has no picture area`);
  return box;
}

/**
 * The start of the lower part of a clip's body on the timeline (under its name), where the
 * filmstrip tiles and the waveform are drawn: 200 px from just inside its left end, clear of
 * the notices in the window's corner.
 */
function clipBody(g: TimelineGeometry, itemId: string): Region {
  const clip = clipBox(g, itemId);
  const left = Math.max(clip.x + 8, g.box.x + g.headerWidth + 4);
  const right = Math.min(clip.x + clip.w - 8, g.box.x + g.box.width - 4, left + 200);
  return { x: left, y: clip.y + 1, width: right - left, height: 16 };
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
      // sound of those that have some (the HEVC clip is silent: no waveform job). Its proxy
      // job (10-bit HEVC, WP-2F.01) is not analysis: F41 follows proxies.
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
                kind: "analysis",
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

      // The cards show a real picture of each file, not the icon tile.
      for (const name of [H264, HEVC, PHONE]) {
        await expect(mediaCard(page, name).locator("img.cw-media__thumb-img")).toBeVisible({
          timeout: 30_000,
        });
        await expect
          .poll(async () => (await pixels(page, await cardThumb(page, name))).dominant, {
            timeout: 30_000,
            message: `the thumbnail of ${name}`,
          })
          .toBeLessThan(0.6);
        expect((await pixels(page, await cardThumb(page, name))).colours).toBeGreaterThan(24);
      }
      await snapshot(page, info, "thumbnails");

      // Place the HEVC clip, then the phone clip right after it.
      await dropCard(page, HEVC, "V1", 0);
      await expect.poll(async () => (await layout(page))[0]?.clips.length ?? 0).toBe(1);
      await editorCaughtUp(page);
      const first = (await layout(page))[0]?.clips[0]?.[1] ?? 0;
      expect(first).toBeCloseTo(5, 1);
      await dropCard(page, PHONE, "V1", first);
      await expect.poll(async () => (await layout(page))[0]?.clips.length ?? 0).toBe(2);
      await editorCaughtUp(page);

      // The video clip draws a filmstrip of its frames; the phone clip's sound (its linked
      // audio clip) draws a waveform. Without them both bodies are flat.
      const placed = await timelineGeometry(page);
      const video = placed.tracks.find((t) => t.name === "V1")?.items[0];
      const sound = placed.tracks
        .find((t) => t.kind === "audio" && t.items.length > 0)
        ?.items.find((i) => Math.abs(i.start - sec(first)) < sec(0.05));
      if (video === undefined || sound === undefined) {
        throw new Error("the HEVC clip or the phone clip's sound is not on the timeline");
      }
      await expect
        .poll(async () => (await pixels(page, clipBody(placed, video.id))).dominant, {
          timeout: 30_000,
          message: "the filmstrip of the HEVC clip",
        })
        .toBeLessThan(0.6);
      expect((await pixels(page, clipBody(placed, video.id))).colours).toBeGreaterThan(24);
      await expect
        .poll(async () => (await pixels(page, clipBody(placed, sound.id))).dominant, {
          timeout: 30_000,
          message: "the waveform of the phone clip's sound",
        })
        .toBeLessThan(0.97);
      await snapshot(page, info, "filmstrip and waveform");

      // The preview streams the real frames.
      const surface = page.locator(".cw-preview__surface");
      await expect(surface).toHaveAttribute("data-path", /^(gpu|jpeg)$/, { timeout: 20_000 });
      test.skip(
        (await surface.getAttribute("data-path")) !== "gpu",
        "the preview fell back to still frames (JPEG); the playback and step checks are for the frame ring",
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
