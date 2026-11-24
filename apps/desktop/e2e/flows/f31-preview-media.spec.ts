// SPDX-License-Identifier: GPL-3.0-or-later
// F31 Preview media before placing it (docs/user-flows.md; WP-1M.14 is the picture half): import
// the corpus clips, move the pointer across the HEVC card (its picture and time change with the
// pointer, and the poster comes back when it leaves), select the card (the Inspector shows its
// picture), drag the Inspector's scrub bar to the middle (the picture is the frame at half the
// clip's length, compared with `/media/{id}/thumb` at that time), step with the keys, and turn
// hover-scrub off in Settings (hovering leaves the poster). All checked from the pixels on screen.
//
// Needs an engine built with FFmpeg and the fetched corpus (`just corpus`); without either the
// flow skips and says why.
import { createHash } from "node:crypto";
import type { Locator, Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import { copyCorpusClips, corpusDir } from "./export-helpers.ts";
import {
  closeSettings,
  createProject,
  hasRealEngine,
  launchFlowApp,
  mediaCard,
  mediaCards,
  openSettingsPage,
  pickFiles,
  snapshot,
  workspace,
} from "./helpers.ts";

const H264 = "h264_1080p60.mp4";
const HEVC = "hevc_10bit_sdr.mp4";

/** The scrub picture of a card: a hash of the pixels the card's canvas shows. */
async function cardPicture(card: Locator): Promise<string | null> {
  return card.evaluate((el) => {
    const canvas = el.querySelector<HTMLCanvasElement>("canvas.cw-media__scrub-canvas");
    if (canvas === null || canvas.hidden) return null;
    const ctx = canvas.getContext("2d");
    if (ctx === null) return null;
    const { data } = ctx.getImageData(0, 0, canvas.width, canvas.height);
    let hash = 5381;
    for (let i = 0; i < data.length; i += 7) hash = ((hash << 5) + hash + (data[i] ?? 0)) | 0;
    return String(hash);
  });
}

/** Resolves once `read` gives the same non-null answer twice in a row. */
async function settled(read: () => Promise<string | null>): Promise<string> {
  let last: string | null = null;
  let value: string | null = null;
  await expect
    .poll(
      async () => {
        value = await read();
        const same = value !== null && value === last;
        last = value;
        return same;
      },
      { intervals: [100, 200, 300], timeout: 20_000 },
    )
    .toBe(true);
  return value ?? "";
}

/** Moves the pointer to `fraction` of the card's picture, as a person would. */
async function hoverAt(page: Page, card: Locator, fraction: number): Promise<void> {
  const box = await card.locator(".cw-media__thumb").boundingBox();
  if (box === null) throw new Error("the card has no picture area");
  const y = box.y + box.height / 2;
  await page.mouse.move(box.x + box.width * fraction, y, { steps: 4 });
}

/**
 * The mean difference (0..255) between the Inspector's picture and the engine's frame at `t`
 * (flicks), both reduced to 96 × 54 pixels: the same frame whatever the size it was drawn at.
 */
async function differenceFromThumb(page: Page, mediaId: string, t: number): Promise<number> {
  return page.evaluate(
    async ({ id, at }) => {
      const canvas = document.querySelector<HTMLCanvasElement>(
        '[data-testid="media-preview-picture"]',
      );
      if (canvas === null) throw new Error("no Inspector picture");
      const connection = await (
        window as unknown as {
          cutweave: {
            engine: {
              getConnection(): Promise<{ httpUrl?: string; port?: number; token: string }>;
            };
          };
        }
      ).cutweave.engine.getConnection();
      const base = connection.httpUrl ?? `http://127.0.0.1:${connection.port}`;
      const response = await fetch(`${base}/media/${id}/thumb?t=${at}&w=640`, {
        headers: { Authorization: `Bearer ${connection.token}` },
      });
      if (!response.ok) throw new Error(`thumb ${response.status}`);
      const frame = await createImageBitmap(await response.blob());
      const reduce = (source: CanvasImageSource): Uint8ClampedArray => {
        const small = new OffscreenCanvas(96, 54);
        const ctx = small.getContext("2d");
        if (ctx === null) throw new Error("no 2D context");
        ctx.drawImage(source, 0, 0, 96, 54);
        return ctx.getImageData(0, 0, 96, 54).data;
      };
      const a = reduce(canvas);
      const b = reduce(frame);
      let sum = 0;
      for (let i = 0; i < a.length; i += 4) {
        sum +=
          Math.abs((a[i] ?? 0) - (b[i] ?? 0)) +
          Math.abs((a[i + 1] ?? 0) - (b[i + 1] ?? 0)) +
          Math.abs((a[i + 2] ?? 0) - (b[i + 2] ?? 0));
      }
      return sum / ((a.length / 4) * 3);
    },
    { id: mediaId, at: t },
  );
}

test.describe("F31 preview media before placing it", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("hover across a card, scrub the Inspector's picture, turn hover-scrub off", async () => {
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
      await createProject(app, page, "Preview", work.dir);

      // Import the clips and wait until the engine has analysed them (the sprite is cached then).
      const files = copyCorpusClips(corpus ?? "", work.media, [H264, HEVC]);
      await pickFiles(app, files);
      await page.getByRole("button", { name: "Import", exact: true }).click();
      await expect(mediaCards(page)).toHaveCount(2);
      const media = await editorCall<{ media: { id: string; path: string }[] }>(page, "media.list");
      for (const asset of media.media) {
        await expect
          .poll(
            async () => {
              const { jobs } = await editorCall<{ jobs: { label: string; state: string }[] }>(
                page,
                "job.list",
                { media_id: asset.id },
              );
              return jobs.some((j) => j.label === "thumbnails" && j.state === "succeeded");
            },
            { timeout: 120_000, message: `the analysis of ${asset.path}` },
          )
          .toBe(true);
      }
      const hevc = mediaCard(page, HEVC);
      await expect(hevc.locator("img.cw-media__thumb-img")).toBeVisible({ timeout: 30_000 });
      const hevcId = media.media.find((m) => m.path.endsWith(HEVC))?.id;
      if (hevcId === undefined) throw new Error("the HEVC clip is not in the bin");

      // Hover across the card: the picture and the time follow the pointer.
      const time = hevc.locator(".cw-media__scrub-time");
      await hoverAt(page, hevc, 0.1);
      const early = await settled(() => cardPicture(hevc));
      await expect(time).toBeVisible();
      const earlyTime = await time.textContent();
      await hoverAt(page, hevc, 0.5);
      const middle = await settled(() => cardPicture(hevc));
      await snapshot(page, info, "hover-scrub-middle");
      const middleTime = await time.textContent();
      await hoverAt(page, hevc, 0.9);
      const late = await settled(() => cardPicture(hevc));
      const lateTime = await time.textContent();
      expect(new Set([early, middle, late]).size).toBe(3);
      expect(new Set([earlyTime, middleTime, lateTime]).size).toBe(3);
      // The clip is 5 s at 30 fps: the middle of the card is 2.5 s.
      expect(middleTime).toMatch(/^00:00:02:1[4-6]$/);
      // The position line follows the pointer.
      const box = await hevc.locator(".cw-media__thumb").boundingBox();
      const line = await hevc.locator(".cw-media__scrub-line").boundingBox();
      if (box === null || line === null) throw new Error("no picture area or line");
      expect((line.x - box.x) / box.width).toBeGreaterThan(0.85);

      // Leaving puts the poster back.
      await page.mouse.move(box.x + box.width / 2, box.y - 200, { steps: 4 });
      await expect(hevc.locator("canvas.cw-media__scrub-canvas")).toBeHidden();
      await expect(time).toBeHidden();
      await expect(hevc.locator("img.cw-media__thumb-img")).toBeVisible();

      // Select the card: the Inspector shows its picture, then the frame at half its length.
      await hevc.click();
      const preview = page.getByTestId("media-preview");
      await expect(preview).toBeVisible();
      const bar = page.getByRole("slider", { name: "Scrub position" });
      await expect(bar).toHaveAttribute("aria-valuenow", "0");
      await expect
        .poll(() => preview.getByTestId("media-preview-picture").getAttribute("data-frame"), {
          timeout: 30_000,
        })
        .toBe("ready");
      expect(await differenceFromThumb(page, hevcId, 0)).toBeLessThan(14);
      await snapshot(page, info, "inspector-preview-start");

      const barBox = await bar.boundingBox();
      if (barBox === null) throw new Error("the scrub bar has no box");
      const y = barBox.y + barBox.height / 2;
      await page.mouse.move(barBox.x + barBox.width * 0.2, y);
      await page.mouse.down();
      await page.mouse.move(barBox.x + barBox.width * 0.35, y, { steps: 4 });
      await page.mouse.move(barBox.x + barBox.width * 0.5, y, { steps: 4 });
      await page.mouse.up();
      const frames = Number(await bar.getAttribute("aria-valuemax")) + 1;
      const frame = Number(await bar.getAttribute("aria-valuenow"));
      const flicksPerFrame = 705_600_000 / 30;
      const at = frame * flicksPerFrame;
      expect(frame).toBe(Math.floor(frames / 2));
      await expect(bar).toHaveAttribute("aria-valuetext", /^00:00:02:1[4-6]$/);
      await expect
        .poll(() => differenceFromThumb(page, hevcId, at), {
          timeout: 30_000,
          message: "the Inspector's picture is the frame at half the clip's length",
        })
        .toBeLessThan(14);
      // It is that frame and not the first or the last: those differ from it clearly.
      const here = await differenceFromThumb(page, hevcId, at);
      const atStart = await differenceFromThumb(page, hevcId, 0);
      const last = await differenceFromThumb(page, hevcId, (frames - 1) * flicksPerFrame);
      expect(here).toBeLessThan(Math.min(atStart, last) * 0.7);
      await snapshot(page, info, "inspector-preview-middle");

      // With the bar focused, the arrow keys step a frame and End goes to the last one.
      await bar.focus();
      await page.keyboard.press("ArrowRight");
      await expect(bar).toHaveAttribute("aria-valuenow", String(frame + 1));
      await page.keyboard.press("End");
      await expect(bar).toHaveAttribute("aria-valuenow", String(frames - 1));
      await page.keyboard.press("Home");
      await expect(bar).toHaveAttribute("aria-valuetext", "00:00:00:00");

      // Settings > Media and import: with hover-scrub off, hovering leaves the poster.
      const settings = await openSettingsPage(page, app, "Media and import");
      const toggle = settings.getByRole("switch", { name: "Preview video on hover" });
      await expect(toggle).toBeChecked();
      await toggle.click();
      await expect(toggle).not.toBeChecked();
      await closeSettings(settings);
      await hoverAt(page, hevc, 0.3);
      await hoverAt(page, hevc, 0.6);
      await expect(hevc.locator("canvas.cw-media__scrub-canvas")).toHaveCount(0);
      await expect(hevc.locator("img.cw-media__thumb-img")).toBeVisible();
      await expect(hevc.locator(".cw-media__duration")).toBeVisible();
      // The picture of the card is the poster: the same pixels at both positions.
      const poster = async () =>
        createHash("sha1")
          .update(await hevc.locator(".cw-media__thumb").screenshot({ animations: "allow" }))
          .digest("hex");
      const first = await poster();
      await hoverAt(page, hevc, 0.8);
      expect(await poster()).toBe(first);
    } finally {
      await close();
    }
  });
});
