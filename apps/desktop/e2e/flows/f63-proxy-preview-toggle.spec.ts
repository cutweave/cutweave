// SPDX-License-Identifier: GPL-3.0-or-later
// F63 Proxies in the preview (docs/user-flows.md; WP-2F.02), as a person does it: import the
// corpus 4K long-GOP clip and wait for its proxy; place it and click the ruler: the chip in the
// transport bar says the preview shows the "Proxy". Turn "Use proxies in the preview" off: the
// same frame is decoded from the file itself and the chip says "Original". Turn it on again:
// "Proxy". Turn it off, quit and relaunch: the toggle is still off, the engine has the choice
// and the chip says "Original"; turned on, it says "Proxy" again.
//
// Needs an engine built with FFmpeg and the fetched corpus (`just corpus`); without either the
// flow skips and says why.
import type { Locator, Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import { copyCorpusClips, corpusDir } from "./export-helpers.ts";
import {
  chooseMenu,
  createProject,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  launchFlowApp,
  layout,
  mediaCard,
  mediaCards,
  openRecent,
  pickFiles,
  profileDir,
  removeDir,
  rulerY,
  saveStatus,
  sec,
  snapshot,
  timecode,
  timelineGeometry,
  workspace,
  xAt,
} from "./helpers.ts";

const CLIP = "h264_4k30_longgop.mp4";

/** What the engine says about the preview's proxies (the command without a change reads it). */
interface PreviewState {
  readonly use_proxies: boolean;
  readonly proxy: boolean;
}

const toggle = (page: Page): Locator =>
  page.getByRole("button", { name: "Use proxies in the preview", exact: true });
/** The transport bar's chip: what the frame in the preview was decoded from. */
const chip = (page: Page, source: "Proxy" | "Original"): Locator =>
  page.getByLabel(`Preview source: ${source}`, { exact: true });
const engineState = (page: Page): Promise<PreviewState> =>
  editorCall<PreviewState>(page, "view.set_preview_quality");

/** Clicks the ruler at `seconds` and waits for the playhead to be there. */
async function seekTo(page: Page, seconds: number, shown: string): Promise<void> {
  const g = await timelineGeometry(page);
  await page.mouse.click(xAt(g, sec(seconds)), rulerY(g));
  await expect(timecode(page)).toHaveText(shown);
}

test.describe("F63 proxies in the preview", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("the chip says Proxy or Original, the toggle switches, and the choice survives a relaunch", async () => {
    test.setTimeout(600_000);
    const info = test.info();
    const corpus = corpusDir();
    test.skip(corpus === null, "the corpus is not fetched (just corpus)");
    const work = workspace();
    const home = profileDir();
    try {
      const first = await launchFlowApp(work, home, { realMedia: true });
      const { app, page } = first;
      const caps = await editorCall<{ available?: boolean }>(page, "media.capabilities");
      test.skip(
        caps.available !== true,
        "this engine has no media backend; build it with --features cutweave-engine/ffmpeg",
      );
      await createProject(app, page, "Proxy preview", work.dir);

      // The toggle is on from the start, and the engine was told so.
      await expect(toggle(page)).toHaveAttribute("aria-pressed", "true");
      await expect.poll(async () => (await engineState(page)).use_proxies).toBe(true);

      // Import the 4K clip; its card says when the proxy is ready.
      const files = copyCorpusClips(corpus ?? "", work.media, [CLIP]);
      await pickFiles(app, files);
      await page.getByRole("button", { name: "Import", exact: true }).click();
      await expect(mediaCards(page)).toHaveCount(1);
      await expect(mediaCard(page, CLIP).getByTestId("proxy-badge")).toHaveAttribute(
        "data-proxy",
        "ready",
        { timeout: 240_000 },
      );

      // Place it and look at a frame: the preview shows the proxy.
      await dropCard(page, CLIP, "V1", 0);
      await expect.poll(async () => (await layout(page))[0]?.clips.length ?? 0).toBe(1);
      await editorCaughtUp(page);
      await expect(page.locator(".cw-preview__surface")).toHaveAttribute(
        "data-path",
        /^(gpu|jpeg)$/,
        { timeout: 20_000 },
      );
      await seekTo(page, 1, "00:00:01:00");
      await expect(chip(page, "Proxy")).toBeVisible({ timeout: 30_000 });
      await expect(chip(page, "Proxy")).toHaveText("Proxy");
      await snapshot(page, info, "proxy");

      // Off: the same frame from the file itself.
      await toggle(page).click();
      await expect(toggle(page)).toHaveAttribute("aria-pressed", "false");
      await expect(chip(page, "Original")).toBeVisible({ timeout: 30_000 });
      await expect(chip(page, "Original")).toHaveText("Original");
      await expect(timecode(page)).toHaveText("00:00:01:00");
      expect((await engineState(page)).use_proxies).toBe(false);
      // Another frame while it is off: still the original.
      await seekTo(page, 3, "00:00:03:00");
      await expect.poll(async () => (await engineState(page)).proxy).toBe(false);
      await expect(chip(page, "Original")).toBeVisible();
      await snapshot(page, info, "original");

      // On again: the proxy.
      await toggle(page).click();
      await expect(toggle(page)).toHaveAttribute("aria-pressed", "true");
      await expect(chip(page, "Proxy")).toBeVisible({ timeout: 30_000 });
      expect((await engineState(page)).use_proxies).toBe(true);

      // Off, then quit: the choice is this computer's and stays.
      await toggle(page).click();
      await expect(toggle(page)).toHaveAttribute("aria-pressed", "false");
      await expect(chip(page, "Original")).toBeVisible({ timeout: 30_000 });
      await expect(saveStatus(page)).toHaveText("All changes saved");
      const closed = app.waitForEvent("close");
      await chooseMenu(page, app, "File", "Exit");
      await closed;
      await first.close();

      const second = await launchFlowApp(work, home, { realMedia: true });
      try {
        const page2 = second.page;
        await expect(toggle(page2)).toHaveAttribute("aria-pressed", "false");
        await expect.poll(async () => (await engineState(page2)).use_proxies).toBe(false);
        await openRecent(page2, "Proxy preview.cutweave");
        await expect(mediaCards(page2)).toHaveCount(1);
        await expect(mediaCard(page2, CLIP).getByTestId("proxy-badge")).toHaveAttribute(
          "data-proxy",
          "ready",
          { timeout: 60_000 },
        );
        await expect.poll(async () => (await layout(page2))[0]?.clips.length ?? 0).toBe(1);
        await editorCaughtUp(page2);
        await seekTo(page2, 2, "00:00:02:00");
        await expect(chip(page2, "Original")).toBeVisible({ timeout: 30_000 });
        await expect.poll(async () => (await engineState(page2)).proxy).toBe(false);

        // And on again in the new session: the proxy, which is still in the cache.
        await toggle(page2).click();
        await expect(toggle(page2)).toHaveAttribute("aria-pressed", "true");
        await expect(chip(page2, "Proxy")).toBeVisible({ timeout: 30_000 });
        await expect.poll(async () => (await engineState(page2)).use_proxies).toBe(true);
        await snapshot(page2, info, "after relaunch");
      } finally {
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
