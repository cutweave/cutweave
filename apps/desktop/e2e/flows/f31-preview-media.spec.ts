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
import { execFile } from "node:child_process";
import { createHash } from "node:crypto";
import path from "node:path";
import { promisify } from "node:util";
import type { ElectronApplication, Locator, Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import { copyCorpusClips, corpusDir } from "./export-helpers.ts";
import {
  chooseMenu,
  chooseTheme,
  cliBin,
  closeSettings,
  createProject,
  dirtyDot,
  dropCard,
  editorCaughtUp,
  FLICKS,
  hasCli,
  hasRealEngine,
  item,
  laneY,
  launchFlowApp,
  layout,
  mediaCard,
  mediaCards,
  movePlayhead,
  openRecent,
  openSettingsPage,
  pickFiles,
  profileDir,
  removeDir,
  rulerY,
  sec,
  sequence,
  shortcut,
  snapshot,
  timecode,
  timelineCanvas,
  timelineGeometry,
  workspace,
  xAt,
} from "./helpers.ts";

const H264 = "h264_1080p60.mp4";
const TRAILER = "sintel_trailer_480p.mp4";
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

/** The source's own time in whole frames at 30 fps (the Source header's timecode). */
async function sourceFrames(page: Page): Promise<number> {
  const text = (await page.getByTestId("source-timecode").textContent()) ?? "";
  const [hh, mm, ss, ff] = text.split(/[:;]/).map(Number);
  return (((hh ?? 0) * 60 + (mm ?? 0)) * 60 + (ss ?? 0)) * 30 + (ff ?? 0);
}

/**
 * A hash of what the Preview shows: a 64 x 36 screenshot at the canvas's centre (a WebGPU canvas
 * cannot be read back from script), away from the overlay.
 */
async function previewHash(page: Page): Promise<string> {
  const box = await page.locator("canvas.cw-preview__canvas").boundingBox();
  if (box === null) throw new Error("no preview canvas");
  const png = await page.screenshot({
    clip: {
      x: Math.round(box.x + box.width / 2 - 32),
      y: Math.round(box.y + box.height / 2 - 18),
      width: 64,
      height: 36,
    },
    animations: "allow",
    caret: "initial",
  });
  return createHash("sha1").update(png).digest("hex");
}

/** A hash of the pixels the Inspector's picture shows. */
async function inspectorHash(page: Page): Promise<string> {
  return page.getByTestId("media-preview-picture").evaluate((el) => {
    const canvas = el as HTMLCanvasElement;
    const ctx = canvas.getContext("2d");
    if (ctx === null) return "";
    const { data } = ctx.getImageData(0, 0, canvas.width, canvas.height);
    let hash = 5381;
    for (let i = 0; i < data.length; i += 7) hash = ((hash << 5) + hash + (data[i] ?? 0)) | 0;
    return String(hash);
  });
}

const run = promisify(execFile);

/** `cutweave exec <step>` attached to the app's engine (its run-file is in the app's HOME). */
async function cli(
  app: ElectronApplication,
  home: string,
  step: { cmd: string; args: Record<string, unknown> },
): Promise<void> {
  const pid = await app.evaluate(() => globalThis.__cutweaveTest?.enginePid() ?? null);
  if (pid === null) throw new Error("no engine is running");
  // Dev stub media in the CLI's own environment would make it spawn an engine of its own.
  const { CUTWEAVE_DEV_STUB_MEDIA: _stub, ...inherited } = process.env;
  await run(
    cliBin,
    [
      "exec",
      "--run-dir",
      path.join(home, ".cutweave", "run"),
      "--pid",
      String(pid),
      JSON.stringify(step),
    ],
    { env: { ...inherited, HOME: home, USERPROFILE: home }, timeout: 30_000 },
  );
}

/** Keys go to the page (the keymap), not to a focused control. */
async function press(page: Page, key: string): Promise<void> {
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press(key);
}

/** Resolves once `read` gives the same number twice in a row. */
async function settledNumber(read: () => Promise<number>): Promise<number> {
  let last: number | null = null;
  let value = 0;
  await expect
    .poll(
      async () => {
        value = await read();
        const same = value === last;
        last = value;
        return same;
      },
      { intervals: [150, 250, 400], timeout: 20_000 },
    )
    .toBe(true);
  return value;
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

  // WP-1M.15: playing a clip before it is placed (ADR-0022). Everything is read from what is on
  // screen: the Source header's timecode, the Preview's pixels, the Inspector's picture.
  test("play a clip from the bin and the Inspector, and place it from the Source view", async () => {
    test.setTimeout(420_000);
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
      await createProject(app, page, "Source", work.dir);
      const files = copyCorpusClips(corpus ?? "", work.media, [TRAILER, HEVC, H264]);
      await pickFiles(app, files);
      await page.getByRole("button", { name: "Import", exact: true }).click();
      await expect(mediaCards(page)).toHaveCount(3);

      // The timeline: the trailer from 0, the playhead at 10 s (the trailer opens on a title).
      await dropCard(page, TRAILER, "V1", 0);
      await expect.poll(async () => (await layout(page))[0]?.clips.length ?? 0).toBe(1);
      await editorCaughtUp(page);
      const surface = page.locator(".cw-preview__surface");
      await expect(surface).toHaveAttribute("data-path", /^(gpu|jpeg)$/, { timeout: 20_000 });
      test.skip(
        (await surface.getAttribute("data-path")) !== "gpu",
        `the preview fell back to still frames (${await surface.getAttribute("data-reason")}); ` +
          "the Source view's frames are the engine's ring",
      );
      await expect(surface).toHaveAttribute("data-live", "true", { timeout: 30_000 });
      const g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(10)), rulerY(g));
      await expect(timecode(page)).toHaveText("00:00:10:00");
      // Click the empty timeline below the tracks: no clip is selected, so no handles (WP-2H.06)
      // draw over the picture whose pixels are compared.
      await page.mouse.click(g.box.x + g.headerWidth + 200, g.box.y + g.box.height - 12);
      await expect(page.locator(".cw-xform")).toHaveCount(0);
      const onTimeline = await settled(() => previewHash(page));
      await shortcut(app, "Control+S");
      await expect(dirtyDot(page)).toBeHidden();
      const layoutBefore = await layout(page);
      const view = await editorCall<{ playhead: number }>(page, "view.get");

      // Press play on the HEVC card: the Preview says "Source" and plays it from 0.
      const hevc = mediaCard(page, HEVC);
      const header = page.getByTestId("source-header");
      const pause = page.getByRole("button", { name: "Pause", exact: true });
      const play = page.getByRole("button", { name: "Play", exact: true });
      await expect(header).toBeHidden();
      await hevc.hover();
      await hevc.locator(".cw-media__play").click();
      await expect(header).toBeVisible();
      await expect(header).toContainText("Source");
      await expect(header).toContainText(HEVC);
      await expect(page.getByTestId("source-duration")).toHaveText("00:00:05:00");
      await expect(pause).toBeVisible();
      // Its own time runs from 0, in the Source header and on the transport.
      await expect.poll(() => sourceFrames(page), { timeout: 20_000 }).toBeGreaterThan(10);
      await expect(timecode(page)).toHaveText(/^00:00:0[0-5]:\d\d$/);
      // The picture is the clip, not the timeline's frame, and it moves.
      const first = await previewHash(page);
      expect(first).not.toBe(onTimeline);
      await expect.poll(() => previewHash(page), { timeout: 20_000 }).not.toBe(first);
      await snapshot(page, info, "source-playing");
      // The timeline's playhead did not move.
      expect((await editorCall<{ playhead: number }>(page, "view.get")).playhead).toBe(
        view.playhead,
      );
      expect(await page.getByRole("radiogroup", { name: "Preview aspect ratio" }).count()).toBe(0);

      // K pauses the source and J, L, → act on it, not on the timeline.
      await press(page, "k");
      await expect(play).toBeVisible();
      const paused = await settledNumber(() => sourceFrames(page));
      await press(page, "ArrowRight");
      await expect.poll(() => sourceFrames(page)).toBe(paused + 1);
      await press(page, "l");
      await expect(pause).toBeVisible();
      await expect.poll(() => sourceFrames(page), { timeout: 20_000 }).toBeGreaterThan(paused + 4);
      await press(page, "k");
      const forward = await settledNumber(() => sourceFrames(page));
      await press(page, "j");
      await expect.poll(() => sourceFrames(page), { timeout: 20_000 }).toBeLessThan(forward - 3);
      await press(page, "k");
      // Go to start / end are the source's own.
      await page.getByRole("button", { name: "Go to end" }).click();
      await expect(header.getByTestId("source-timecode")).toHaveText("00:00:04:29");
      await page.getByRole("button", { name: "Go to start" }).click();
      await expect(header.getByTestId("source-timecode")).toHaveText("00:00:00:00");
      expect((await editorCall<{ playhead: number }>(page, "view.get")).playhead).toBe(
        view.playhead,
      );

      // Back to timeline: the timeline frame at the playhead it had.
      await header.getByRole("button", { name: "Back to timeline" }).click();
      await expect(header).toBeHidden();
      await expect(timecode(page)).toHaveText("00:00:10:00");
      await expect.poll(() => previewHash(page), { timeout: 20_000 }).toBe(onTimeline);
      expect((await editorCall<{ playhead: number }>(page, "view.get")).playhead).toBe(
        view.playhead,
      );

      // Play from the Inspector: its picture changes, the Preview's does not.
      await hevc.click();
      const picture = page.getByTestId("media-preview-picture");
      await expect(picture).toHaveAttribute("data-frame", "ready", { timeout: 30_000 });
      await page.getByTestId("media-preview-play").click();
      await expect(page.getByTestId("media-preview-play")).toHaveAccessibleName(/^Pause /);
      const still = await settled(() => inspectorHash(page));
      await expect.poll(() => inspectorHash(page), { timeout: 20_000 }).not.toBe(still);
      await expect(header).toBeHidden();
      for (let i = 0; i < 4; i += 1) {
        expect(await previewHash(page)).toBe(onTimeline);
        await page.waitForTimeout(250);
      }
      const bar = page.getByRole("slider", { name: "Scrub position" });
      await expect
        .poll(async () => Number(await bar.getAttribute("aria-valuenow")), { timeout: 20_000 })
        .toBeGreaterThan(10);
      await snapshot(page, info, "inspector-playing");
      await page.getByTestId("media-preview-play").click();
      await expect(page.getByTestId("media-preview-play")).toHaveAccessibleName(/^Play /);

      // Starting it from the bin moves it to the Preview.
      await hevc.hover();
      await hevc.locator(".cw-media__play").click();
      await expect(header).toBeVisible();
      await press(page, "k");
      await expect(play).toBeVisible();

      // Enter in the Source view inserts the whole clip at the timeline's playhead: one step.
      await page.getByRole("group", { name: /^Source: / }).click();
      await page.keyboard.press("Enter");
      const clipCount = async () => (await layout(page)).flatMap((t) => t.clips).length;
      const clipsBefore = layoutBefore.flatMap((t) => t.clips).length;
      await expect.poll(clipCount).toBeGreaterThan(clipsBefore);
      const placed = (await layout(page))
        .flatMap((t) => t.clips)
        .find(([start, length]) => Math.abs(start - 10) < 0.1 && Math.abs(length - 5) < 0.1);
      expect(placed).toBeDefined();
      expect(placed?.[0]).toBeCloseTo(10, 1);
      expect(placed?.[1]).toBeCloseTo(5, 1);
      await expect(dirtyDot(page)).toBeVisible();
      // The source stays up and the timeline's playhead is where it was.
      await expect(header).toBeVisible();
      expect((await editorCall<{ playhead: number }>(page, "view.get")).playhead).toBe(
        view.playhead,
      );
      await snapshot(page, info, "source-placed");

      // Esc goes back; one undo removes the clip and the project is as it was saved.
      await page.getByRole("group", { name: /^Source: / }).focus();
      await page.keyboard.press("Escape");
      await expect(header).toBeHidden();
      await shortcut(app, "Control+Z");
      await expect.poll(() => layout(page)).toEqual(layoutBefore);
      await expect(dirtyDot(page)).toBeHidden();

      // Dragging the Source view's picture onto the timeline drops the clip like a card.
      await hevc.hover();
      await hevc.locator(".cw-media__play").click();
      await expect(header).toBeVisible();
      await press(page, "k");
      const g2 = await timelineGeometry(page);
      await page.locator(".cw-preview__frame").dragTo(timelineCanvas(page), {
        targetPosition: { x: xAt(g2, sec(6)) - g2.box.x, y: laneY(g2, "V1") - g2.box.y },
      });
      await expect.poll(clipCount).toBeGreaterThan(clipsBefore);

      // The header in both themes (a picture to look at, as for every key step).
      await snapshot(page, info, "source-header-dark");
      await chooseTheme(page, app, "Light");
      await expect(header).toBeVisible();
      await snapshot(page, info, "source-header-light");
      await chooseTheme(page, app, "Dark");

      // Touching the timeline leaves the Source view.
      await page.mouse.click(xAt(g2, sec(2)), rulerY(g2));
      await expect(header).toBeHidden();
    } finally {
      await close();
    }
  });

  test("mark a source, insert and overwrite the range, reopen, match frame (WP-2A.14)", async () => {
    test.setTimeout(420_000);
    const info = test.info();
    const corpus = corpusDir();
    test.skip(corpus === null, "the corpus is not fetched (just corpus)");
    const work = workspace();
    const home = profileDir();
    try {
      const first = await launchFlowApp(work, home, { realMedia: true });
      const { app, page } = first;
      let closedFirst = false;
      try {
        const caps = await editorCall<{ available?: boolean }>(page, "media.capabilities");
        test.skip(
          caps.available !== true,
          "this engine has no media backend; build it with --features cutweave-engine/ffmpeg",
        );
        await createProject(app, page, "Marks", work.dir);
        const files = copyCorpusClips(corpus ?? "", work.media, [TRAILER]);
        await pickFiles(app, files);
        await page.getByRole("button", { name: "Import", exact: true }).click();
        await expect(mediaCards(page)).toHaveCount(1);
        const card = mediaCard(page, TRAILER);
        const header = page.getByTestId("source-header");
        const sourceTime = page.getByTestId("source-timecode");
        const bar = page.getByRole("slider", { name: "Source position" });
        const marksText = page.getByTestId("source-marks");
        const cardRange = card.getByTestId("card-marked-range");
        const stage = page.getByRole("group", { name: /^Source: / });
        /** Opens the trailer in the Source view from its card and pauses it. */
        const openSource = async () => {
          await card.hover();
          await card.locator(".cw-media__play").click();
          await expect(header).toBeVisible();
          await press(page, "k");
          await expect(page.getByRole("button", { name: "Play", exact: true })).toBeVisible();
        };
        /** Moves the source's playhead to a whole second with the scrub bar's keys. */
        const goToSecond = async (seconds: number) => {
          await bar.focus();
          await page.keyboard.press("Home");
          for (let i = 0; i < seconds; i += 1) await page.keyboard.press("PageUp");
          await expect(sourceTime).toHaveText(`00:00:${String(seconds).padStart(2, "0")}:00`);
        };
        /** The clips of the first video track: start, length and media position, in seconds. */
        const placed = async () => {
          const track = (await sequence(page)).tracks.find((t) => t.kind === "video");
          const out: [number, number, number][] = [];
          for (const clip of track?.items ?? []) {
            const got = await item(page, clip.id);
            out.push([
              clip.start / FLICKS,
              clip.duration / FLICKS,
              (got.source?.source_in ?? -1) / FLICKS,
            ]);
          }
          return out;
        };
        /** A clip as the engine has it, without its identity: what parity compares. */
        const normalized = async () => {
          const track = (await sequence(page)).tracks.find((t) => t.kind === "video");
          const got = await item(page, track?.items[0]?.id ?? "");
          // Its own id and its link group's are new each time; that it has a group is compared.
          const {
            id: _id,
            linked_group: group,
            ...rest
          } = got.item as typeof got.item & {
            linked_group?: string | null;
          };
          return { ...got, item: { ...rest, linked: group != null }, linked: got.linked.length };
        };

        // Play it in the Source view, mark in at 2 s and out at 5 s: on the bar and on its card.
        await openSource();
        await expect(marksText).toBeHidden();
        await expect(cardRange).toHaveCount(0);
        await goToSecond(2);
        await page.keyboard.press("i");
        await goToSecond(5);
        await page.keyboard.press("o");
        await expect(marksText).toHaveText("In 00:00:02:00 · Out 00:00:05:00 · 00:00:03:00");
        await expect(page.getByTestId("source-range")).toBeVisible();
        await expect(cardRange).toBeVisible();
        await expect(cardRange).toHaveAttribute("data-in", String(sec(2)));
        await expect(cardRange).toHaveAttribute("data-out", String(sec(5)));
        // The engine holds them with the project's view state, by asset.
        await expect
          .poll(async () =>
            Object.values(
              (await editorCall<{ source_marks: Record<string, object> }>(page, "view.get"))
                .source_marks,
            ),
          )
          .toEqual([{ in: sec(2), out: sec(5) }]);
        await snapshot(page, info, "source-marked");
        // Marks are not edits: the project is still clean.
        await expect(dirtyDot(page)).toBeHidden();

        // `,` inserts the range at the playhead (0 s): a 3 s clip that starts at source 2 s.
        await stage.click();
        await page.keyboard.press(",");
        await expect.poll(placed).toEqual([[0, 3, 2]]);
        await editorCaughtUp(page);
        await expect(header).toBeVisible();
        const fromSourceView = await normalized();

        // One undo step; the same `timeline.insert` through the CLI makes the same clip (parity).
        await shortcut(app, "Control+Z");
        await expect.poll(placed).toEqual([]);
        if (hasCli) {
          const seq = await sequence(page);
          const media = await editorCall<{ media: { id: string }[] }>(page, "media.list", {});
          await cli(app, home, {
            cmd: "timeline.insert",
            args: {
              sequence: seq.id,
              track: seq.tracks.find((t) => t.kind === "video")?.id,
              media: media.media[0]?.id,
              at: 0,
              source_in: sec(2),
              duration: sec(3),
              mode: "insert",
            },
          });
          await expect.poll(placed).toEqual([[0, 3, 2]]);
          expect(await normalized()).toEqual(fromSourceView);
        } else {
          info.annotations.push({ type: "note", description: "no CLI built: parity not compared" });
          await stage.click();
          await page.keyboard.press(",");
          await expect.poll(placed).toEqual([[0, 3, 2]]);
        }
        await editorCaughtUp(page);

        // Put the playhead on the clip (this leaves the Source view), come back: the marks stayed.
        await movePlayhead(page, 1);
        await expect(header).toBeHidden();
        await openSource();
        await expect(marksText).toHaveText("In 00:00:02:00 · Out 00:00:05:00 · 00:00:03:00");
        // New marks, 10 s to 11 s, and `.`: one second of the clip under the playhead is replaced.
        await goToSecond(10);
        await page.keyboard.press("i");
        await goToSecond(11);
        await page.keyboard.press("o");
        await expect(marksText).toHaveText("In 00:00:10:00 · Out 00:00:11:00 · 00:00:01:00");
        await stage.click();
        await page.keyboard.press(".");
        await expect.poll(placed).toEqual([
          [0, 1, 2],
          [1, 1, 10],
          [2, 1, 4],
        ]);
        await editorCaughtUp(page);
        await snapshot(page, info, "source-overwritten");

        // Save, quit and reopen the project: the marks are still on the card.
        await shortcut(app, "Control+S");
        await expect(dirtyDot(page)).toBeHidden();
        const closed = app.waitForEvent("close");
        await chooseMenu(page, app, "File", "Exit");
        await closed;
        closedFirst = true;
        await first.close();
      } finally {
        if (!closedFirst) await first.close();
      }

      const second = await launchFlowApp(work, home, { realMedia: true });
      try {
        const page = second.page;
        await openRecent(page, "Marks.cutweave");
        await expect(page).toHaveTitle("Marks — Cutweave");
        await expect(mediaCards(page)).toHaveCount(1);
        const cardRange = mediaCard(page, TRAILER).getByTestId("card-marked-range");
        await expect(cardRange).toBeVisible();
        await expect(cardRange).toHaveAttribute("data-in", String(sec(10)));
        await expect(cardRange).toHaveAttribute("data-out", String(sec(11)));
        await editorCaughtUp(page);
        await snapshot(page, info, "card-marked-after-reopen");

        // F with the playhead on the last piece (2 s to 3 s, source 4 s to 5 s): the Source view
        // opens at the same frame, with the piece's source range as its marks.
        await movePlayhead(page, 2.5);
        await page.keyboard.press("f");
        const header = page.getByTestId("source-header");
        await expect(header).toBeVisible();
        await expect(header).toContainText(TRAILER);
        await expect(page.getByTestId("source-timecode")).toHaveText("00:00:04:12");
        await expect(page.getByTestId("source-marks")).toHaveText(
          "In 00:00:04:00 · Out 00:00:05:00 · 00:00:01:00",
        );
        // The timeline's playhead stayed where it was.
        expect((await editorCall<{ playhead: number }>(page, "view.get")).playhead).toBe(sec(2.5));
        await snapshot(page, info, "match-frame");
      } finally {
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
