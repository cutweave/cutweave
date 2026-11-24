// SPDX-License-Identifier: GPL-3.0-or-later
// F13 Preview and transport (docs/user-flows.md; WP-1I.05 wires the real engine's playback): a
// corpus clip on the timeline, then the transport like a person drives it: Space plays and
// pauses, J/K/L shuttle, the arrows and the step buttons move one frame, Go to start / Go to end,
// mute and unmute. The timecode and the preview frames move while playing and hold still when
// paused; stepping away and back shows the same picture (frames are rendered per frame, arch §8).
//
// The engine renders every frame (FFmpeg decode, the wgpu compositor) into the preview ring the
// preload presents with WebGPU, exactly as for a person. Needs an engine built with FFmpeg
// (`just e2e-flows` builds it when the bundle is set up) and the fetched corpus (`just corpus`);
// without either the flow skips and says why. The fake-engine version of these checks stays in
// `e2e/preview.spec.ts`.
import { createHash } from "node:crypto";
import type { Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import { copyCorpusClips, corpusDir } from "./export-helpers.ts";
import {
  createProject,
  dropCard,
  editorCaughtUp,
  FRAME,
  hasRealEngine,
  launchFlowApp,
  layout,
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

const CLIP = "sintel_trailer_480p.mp4";

/** `HH:MM:SS:FF` (or `;` for drop-frame) → frames at 30 fps (the new project's rate). */
function frames(text: string | null): number {
  const [hh, mm, ss, ff] = (text ?? "").split(/[:;]/).map(Number);
  return (((hh ?? 0) * 60 + (mm ?? 0)) * 60 + (ss ?? 0)) * 30 + (ff ?? 0);
}

const timecodeFrames = async (page: Page): Promise<number> =>
  frames(await timecode(page).textContent());

/**
 * A hash of what the preview shows: a 64 × 36 screenshot at the canvas's centre (whichever path
 * drew it; a WebGPU canvas cannot be read back from script reliably), away from the overlay.
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

/** Resolves once `read` gives the same answer twice in a row (the value settled). */
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

/** Keys go to the page (the keymap), not to a focused control. */
async function press(page: Page, key: string): Promise<void> {
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press(key);
}

test.describe("F13 preview and transport", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("Space, J/K/L, arrows and the transport buttons move the timecode and the frames", async () => {
    test.setTimeout(240_000);
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
      const play = page.getByRole("button", { name: "Play", exact: true });
      const pause = page.getByRole("button", { name: "Pause", exact: true });

      // A clip on the timeline.
      await createProject(app, page, "Play", work.dir);
      const files = copyCorpusClips(corpus ?? "", work.media, [CLIP]);
      await pickFiles(app, files);
      await page.getByRole("button", { name: "Import", exact: true }).click();
      await expect(mediaCards(page)).toHaveCount(1);
      await dropCard(page, CLIP, "V1", 0);
      await expect.poll(async () => (await layout(page))[0]?.clips.length ?? 0).toBe(1);
      await editorCaughtUp(page);

      // The preview streams real frames through the ring.
      const surface = page.locator(".cw-preview__surface");
      await expect(surface).toHaveAttribute("data-path", /^(gpu|jpeg)$/, { timeout: 20_000 });
      const path = await surface.getAttribute("data-path");
      test.skip(
        path !== "gpu",
        `the preview fell back to still frames (${await surface.getAttribute("data-reason")}); ` +
          "the JPEG path needs /media/frame (WP-1I.03)",
      );
      await expect(surface).toHaveAttribute("data-live", "true", { timeout: 30_000 });
      await expect(play).toBeEnabled();
      await expect(timecode(page)).toHaveText("00:00:00:00");

      // Go somewhere with motion (the trailer opens on a still title): click the ruler at 10 s.
      const g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(10)), rulerY(g));
      await expect(timecode(page)).toHaveText("00:00:10:00");
      const atTen = await settled(() => previewHash(page));

      // Space plays: the timecode runs and the picture changes; Space again pauses.
      await press(page, "Space");
      await expect(pause).toBeVisible();
      await expect.poll(() => timecodeFrames(page), { timeout: 20_000 }).toBeGreaterThan(300 + 15);
      await expect.poll(() => previewHash(page)).not.toBe(atTen);
      await snapshot(page, info, "playing");
      await press(page, "Space");
      await expect(play).toBeVisible();
      const paused = await settled(() => timecodeFrames(page));
      expect(paused).toBeGreaterThan(300);
      const pausedHash = await settled(() => previewHash(page));
      // The engine's view playhead is where playback stopped (inside the frame shown).
      const view = await editorCall<{ playhead: number }>(page, "view.get");
      expect(Math.floor(view.playhead / FRAME)).toBe(paused);

      // The arrows step one frame at a time; each frame has its picture, and stepping back
      // over the same frames shows the same pictures again (frame-accurate preview).
      const forwardHashes: string[] = [];
      for (let i = 1; i <= 5; i += 1) {
        await press(page, "ArrowRight");
        await expect.poll(() => timecodeFrames(page)).toBe(paused + i);
        forwardHashes.push(await settled(() => previewHash(page)));
      }
      // The trailer is 24 fps on a 30 fps timeline: five frames show at least three pictures.
      expect(new Set(forwardHashes).size).toBeGreaterThanOrEqual(3);
      for (let i = 4; i >= 1; i -= 1) {
        await press(page, "ArrowLeft");
        await expect.poll(() => timecodeFrames(page)).toBe(paused + i);
        await expect.poll(() => previewHash(page)).toBe(forwardHashes[i - 1]);
      }
      // Back where playback paused: the paused frame's picture.
      await press(page, "ArrowLeft");
      await expect.poll(() => timecodeFrames(page)).toBe(paused);
      await expect.poll(() => previewHash(page)).toBe(pausedHash);

      // L plays forward, K pauses; J plays backwards, K pauses.
      await press(page, "l");
      await expect(pause).toBeVisible();
      await expect
        .poll(() => timecodeFrames(page), { timeout: 20_000 })
        .toBeGreaterThan(paused + 10);
      await press(page, "k");
      await expect(play).toBeVisible();
      const forward = await settled(() => timecodeFrames(page));
      await press(page, "j");
      await expect(pause).toBeVisible();
      await expect.poll(() => timecodeFrames(page), { timeout: 20_000 }).toBeLessThan(forward - 5);
      await press(page, "k");
      await expect(play).toBeVisible();
      await settled(() => timecodeFrames(page));

      // The buttons: to the start, one frame on, to the end.
      await page.getByRole("button", { name: "Go to start" }).click();
      await expect(timecode(page)).toHaveText("00:00:00:00");
      await page.getByRole("button", { name: "Step forward one frame" }).click();
      await expect(timecode(page)).toHaveText("00:00:00:01");
      await page.getByRole("button", { name: "Step back one frame" }).click();
      await expect(timecode(page)).toHaveText("00:00:00:00");
      await page.getByRole("button", { name: "Go to end" }).click();
      const clipFrames = Math.round(((await layout(page))[0]?.clips[0]?.[1] ?? 0) * 30);
      await expect.poll(() => timecodeFrames(page)).toBe(clipFrames - 1);
      await snapshot(page, info, "at the end");

      // Mute and unmute: the engine's master volume follows.
      await page.getByRole("button", { name: "Mute", exact: true }).click();
      await expect(page.getByRole("button", { name: "Unmute", exact: true })).toBeVisible();
      await expect
        .poll(async () => (await editorCall<{ master_gain: number }>(page, "view.get")).master_gain)
        .toBe(0);
      await page.getByRole("button", { name: "Unmute", exact: true }).click();
      await expect
        .poll(async () => (await editorCall<{ master_gain: number }>(page, "view.get")).master_gain)
        .toBe(1);
    } finally {
      await close();
      work.remove();
    }
  });
});
