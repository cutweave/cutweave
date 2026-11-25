// SPDX-License-Identifier: GPL-3.0-or-later
// F23 Cache and hardware settings (docs/user-flows.md; WP-1D.08), as a person does it: import
// real clips and let the bin draw their thumbnails; File > Settings… > Cache and storage lists
// each cache with its usage and limit; "Clear thumbnails" drops that usage to zero; choose
// another cache folder with the OS folder picker (stubbed: the one dialog the app does not own);
// Performance and hardware names the GPU and the hardware encoders, turns hardware decoding Off
// and picks the memory for decoded frames. Quit and relaunch: the page shows the new folder, the
// engine writes its caches there, and the hardware choices still apply to the new engine.
//
// Needs an engine built with FFmpeg and the fetched corpus (`just corpus`); without either the
// flow skips and says why.
import { existsSync, readdirSync } from "node:fs";
import path from "node:path";
import type { Locator } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import { copyCorpusClips, corpusDir } from "./export-helpers.ts";
import {
  chooseMenu,
  closeSettings,
  createProject,
  hasRealEngine,
  launchFlowApp,
  mediaCard,
  mediaCards,
  openRecent,
  openSettingsPage,
  pickFiles,
  profileDir,
  removeDir,
  saveStatus,
  snapshot,
  workspace,
} from "./helpers.ts";

const HEVC = "hevc_10bit_sdr.mp4";
const PHONE = "vfr_phone_rotated90.mp4";
const GIB = 1024 ** 3;

interface CacheStatus {
  folder: string | null;
  kinds: { kind: string; bytes: number; limit_bytes: number; entries: number }[];
  decode_hw: string;
  frame_cache_bytes: number;
}

/** Files under `dir`, at any depth (0 when it does not exist yet). */
function filesUnder(dir: string): number {
  if (!existsSync(dir)) return 0;
  return readdirSync(dir, { recursive: true, withFileTypes: true }).filter((e) => e.isFile())
    .length;
}

const radio = (group: Locator, name: string): Locator => group.getByRole("radio", { name });
const usage = (settings: Locator, kind: "thumbnails" | "waveforms"): Locator =>
  settings.getByTestId(`cache-usage-${kind}`);

test.describe("F23 cache and hardware settings", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("usage, clear, another cache folder, hardware decoding off; the choices survive a relaunch", async () => {
    test.setTimeout(300_000);
    const info = test.info();
    const corpus = corpusDir();
    test.skip(corpus === null, "the corpus is not fetched (just corpus)");
    const work = workspace();
    const home = profileDir();
    // The folder the person picks; the engine's own default is a throwaway one (launchFlowApp).
    const chosenCache = path.join(work.dir, "chosen-cache");
    const defaultCache = path.join(work.dir, "default-cache");
    try {
      const first = await launchFlowApp(work, home, { realMedia: true });
      const { app, page } = first;
      const caps = await editorCall<{ available?: boolean }>(page, "media.capabilities");
      test.skip(
        caps.available !== true,
        "this engine has no media backend; build it with --features cutweave-engine/ffmpeg",
      );
      const doc = await createProject(app, page, "Cache", work.dir);
      expect(doc).toContain("Cache");

      // The bin draws a real picture of each clip: the thumbnail cache is in use.
      const files = copyCorpusClips(corpus ?? "", work.media, [HEVC, PHONE]);
      await pickFiles(app, files);
      await page.getByRole("button", { name: "Import", exact: true }).click();
      await expect(mediaCards(page)).toHaveCount(2);
      for (const name of [HEVC, PHONE]) {
        await expect(mediaCard(page, name).locator("img.cw-media__thumb-img")).toBeVisible({
          timeout: 60_000,
        });
      }

      // Settings > Cache and storage: the folder in use, each cache's usage and its limit.
      let settings = await openSettingsPage(page, app, "Cache and storage");
      await expect(settings.getByRole("textbox", { name: "Cache folder" })).toHaveValue(
        defaultCache,
      );
      await expect(usage(settings, "thumbnails")).toContainText("in use", { timeout: 60_000 });
      await expect(usage(settings, "waveforms")).toContainText("in use", { timeout: 60_000 });
      await expect(
        settings.getByRole("combobox", { name: "Limit for Thumbnails and filmstrips" }),
      ).toHaveValue(String(2 * GIB));
      await expect(settings.getByRole("combobox", { name: "Limit for Waveforms" })).toHaveValue(
        String(512 * 1024 * 1024),
      );
      await snapshot(page, info, "cache and storage");

      // Clear thumbnails: their usage drops to zero; the waveforms are not touched. The import's
      // background jobs (analysis, thumbnails, proxies) finish first: one still writing after the
      // clear would put a picture back in the cache, which is right but not what this step checks.
      await expect
        .poll(
          async () => {
            const { jobs } = await editorCall<{ jobs: { label: string; state: string }[] }>(
              page,
              "job.list",
              {},
            );
            return jobs
              .filter((j) => j.state === "queued" || j.state === "running")
              .map((j) => `${j.label}:${j.state}`);
          },
          { timeout: 180_000, message: "the import's background jobs" },
        )
        .toEqual([]);
      await settings.getByRole("button", { name: "Clear thumbnails" }).click();
      await expect(usage(settings, "thumbnails")).toHaveText("Empty");
      await expect(usage(settings, "waveforms")).toContainText("in use");
      await expect(settings.getByRole("button", { name: "Clear thumbnails" })).toBeDisabled();
      const status = await editorCall<CacheStatus>(page, "media.cache_status");
      expect(status.kinds.find((k) => k.kind === "thumbnails")).toMatchObject({
        bytes: 0,
        entries: 0,
      });
      expect(status.folder).toBe(defaultCache);

      // Choose another cache folder: it is shown at once and applies the next time Cutweave
      // starts (the engine keeps using the old folder until then).
      await pickFiles(app, [chosenCache]);
      await settings.getByRole("button", { name: "Choose…" }).click();
      await expect(settings.getByRole("textbox", { name: "Cache folder" })).toHaveValue(
        chosenCache,
      );
      await expect(settings.locator('[data-setting="cache.folder"]')).toContainText(
        "Applies at the next start",
      );
      expect((await editorCall<CacheStatus>(page, "media.cache_status")).folder).toBe(defaultCache);

      // Performance and hardware: the GPU in use, the hardware encoders found, hardware
      // decoding Auto by default; turn it Off, and give the preview 2 GB for decoded frames.
      await settings.getByRole("tab", { name: "Performance and hardware" }).click();
      await expect(
        settings.getByRole("heading", { name: "Performance and hardware", level: 3 }),
      ).toBeVisible();
      const gpu = settings.getByTestId("gpu-name");
      await expect(gpu).not.toHaveText("Reading…", { timeout: 30_000 });
      await expect(gpu).toHaveText(/\S/);
      await expect(gpu).not.toContainText("None in use");
      await expect(settings.getByTestId("hw-encode")).not.toHaveText("Reading…");
      const hardware = settings.getByRole("radiogroup", { name: "Hardware decoding" });
      await expect(radio(hardware, "Auto")).toHaveAttribute("aria-checked", "true");
      await radio(hardware, "Off").click();
      await expect(radio(hardware, "Off")).toHaveAttribute("aria-checked", "true");
      await settings
        .getByRole("combobox", { name: "Memory for decoded frames" })
        .selectOption(String(2 * GIB));
      await expect
        .poll(async () => (await editorCall<CacheStatus>(page, "media.cache_status")).decode_hw)
        .toBe("off");
      await expect
        .poll(
          async () => (await editorCall<CacheStatus>(page, "media.cache_status")).frame_cache_bytes,
        )
        .toBe(2 * GIB);
      await snapshot(page, info, "performance and hardware");
      await closeSettings(settings);
      // The import is saved (autosave) before the app is quit, so Exit has nothing to ask.
      await expect(saveStatus(page)).toHaveText("All changes saved");

      const closed = app.waitForEvent("close");
      await chooseMenu(page, app, "File", "Exit");
      await closed;
      await first.close();

      // Relaunch on the same profile: the page shows the new folder, the engine writes there,
      // and the hardware choices apply to the new engine.
      const second = await launchFlowApp(work, home, { realMedia: true });
      try {
        const page2 = second.page;
        await openRecent(page2, "Cache.cutweave");
        await expect(page2).toHaveTitle("Cache — Cutweave");
        await expect(mediaCards(page2)).toHaveCount(2);
        // The bin asks for its pictures again: they are generated into the chosen folder.
        for (const name of [HEVC, PHONE]) {
          await expect(mediaCard(page2, name).locator("img.cw-media__thumb-img")).toBeVisible({
            timeout: 60_000,
          });
        }
        await expect
          .poll(() => filesUnder(path.join(chosenCache, "media-cache")), { timeout: 60_000 })
          .toBeGreaterThan(0);
        expect(existsSync(path.join(chosenCache, "cache.sqlite"))).toBe(true);
        const status2 = await editorCall<CacheStatus>(page2, "media.cache_status");
        expect(status2.folder).toBe(chosenCache);
        expect(status2.decode_hw).toBe("off");
        expect(status2.frame_cache_bytes).toBe(2 * GIB);

        settings = await openSettingsPage(page2, second.app, "Cache and storage");
        await expect(settings.getByRole("textbox", { name: "Cache folder" })).toHaveValue(
          chosenCache,
        );
        await expect(usage(settings, "thumbnails")).toContainText("in use", { timeout: 30_000 });
        await settings.getByRole("tab", { name: "Performance and hardware" }).click();
        await expect(
          radio(settings.getByRole("radiogroup", { name: "Hardware decoding" }), "Off"),
        ).toHaveAttribute("aria-checked", "true");
        await expect(
          settings.getByRole("combobox", { name: "Memory for decoded frames" }),
        ).toHaveValue(String(2 * GIB));
        await closeSettings(settings);
      } finally {
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
