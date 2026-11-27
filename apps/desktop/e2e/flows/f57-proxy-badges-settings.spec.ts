// SPDX-License-Identifier: GPL-3.0-or-later
// F57 Proxies in the bin and in Settings (docs/user-flows.md; WP-2F.03b, ADR-0027), as a person
// does it: Settings shows the proxy choices at their defaults; import the corpus 4K long-GOP
// clip and its card says the proxy is being made, then that it is ready; "Delete proxies" on the
// card's menu removes it ("Proxy removed") and "Regenerate proxies" makes it again; choose
// another proxy cache limit, one proxy job at a time and "All video" for new projects; quit and
// relaunch: the pages show the choices, the engine has them, the card still has its badge and a
// new project starts with the chosen policy.
//
// Needs an engine built with FFmpeg and the fetched corpus (`just corpus`); without either the
// flow skips and says why.
import type { Locator, Page } from "@playwright/test";
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

const CLIP = "h264_4k30_longgop.mp4";
const GIB = 1024 ** 3;

interface CacheStatus {
  kinds: { kind: string; limit_bytes: number; entries: number }[];
  proxy_size: number;
  proxy_jobs: number;
}

interface Proxies {
  proxies: { status: string; wanted: boolean }[];
  policy: string;
}

const badge = (page: Page): Locator => mediaCard(page, CLIP).getByTestId("proxy-badge");
const radio = (group: Locator, name: string): Locator => group.getByRole("radio", { name });
const proxyLimit = (status: CacheStatus): number | undefined =>
  status.kinds.find((k) => k.kind === "proxies")?.limit_bytes;

/** Right-clicks the clip's card and chooses a row of its menu. */
async function cardMenu(page: Page, row: string): Promise<void> {
  await mediaCard(page, CLIP).click({ button: "right" });
  await page.getByRole("menuitem", { name: row }).click();
}

test.describe("F57 proxies in the bin and in Settings", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("a badge follows the proxy; delete and regenerate from the card; the settings survive a relaunch", async () => {
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
      await createProject(app, page, "Proxies", work.dir);

      // Settings: the proxy choices, at their defaults.
      let settings = await openSettingsPage(page, app, "Media and import");
      const policy = settings.getByRole("radiogroup", { name: "Proxies for new projects" });
      await expect(radio(policy, "Auto")).toHaveAttribute("aria-checked", "true");
      await expect(settings.getByRole("combobox", { name: "Proxy size" })).toHaveValue("960");
      await settings.getByRole("tab", { name: "Cache and storage" }).click();
      await expect(settings.getByRole("combobox", { name: "Limit for Proxies" })).toHaveValue(
        String(50 * GIB),
      );
      await settings.getByRole("tab", { name: "Performance and hardware" }).click();
      await expect(settings.getByRole("combobox", { name: "Proxy jobs at once" })).toHaveValue("2");
      await closeSettings(settings);

      // Import the 4K clip: its card says a proxy is on the way, how far it is, then "Proxy".
      const files = copyCorpusClips(corpus ?? "", work.media, [CLIP]);
      await pickFiles(app, files);
      await page.getByRole("button", { name: "Import", exact: true }).click();
      await expect(mediaCards(page)).toHaveCount(1);
      await expect(badge(page)).toHaveAttribute("data-proxy", /^(queued|generating)$/, {
        timeout: 60_000,
      });
      await expect(badge(page)).toHaveText(/^Proxy \d+%$/, { timeout: 120_000 });
      await snapshot(page, info, "proxy generating");
      await expect(badge(page)).toHaveAttribute("data-proxy", "ready", { timeout: 180_000 });
      await expect(badge(page)).toHaveText("Proxy");
      await snapshot(page, info, "proxy ready");
      const made = await editorCall<Proxies>(page, "media.proxy_status", {});
      expect(made.proxies[0]).toMatchObject({ status: "ready", wanted: true });
      expect(made.policy).toBe("auto");

      // Delete proxies on the card's menu: the badge says it is gone, and the engine agrees.
      await cardMenu(page, "Delete proxies");
      await expect(badge(page)).toHaveAttribute("data-proxy", "evicted");
      await expect(badge(page)).toHaveText("Proxy removed");
      expect((await editorCall<Proxies>(page, "media.proxy_status", {})).proxies[0]?.status).toBe(
        "evicted",
      );
      // With nothing left to delete the row is there but not available.
      await mediaCard(page, CLIP).click({ button: "right" });
      await expect(page.getByRole("menuitem", { name: "Delete proxies" })).toBeDisabled();
      // Regenerate proxies, from the same menu: it is made again.
      await page.getByRole("menuitem", { name: "Regenerate proxies" }).click();
      await expect(badge(page)).toHaveAttribute("data-proxy", /^(queued|generating)$/, {
        timeout: 30_000,
      });
      await expect(badge(page)).toHaveAttribute("data-proxy", "ready", { timeout: 180_000 });

      // Settings: a larger proxy cache, one proxy job at a time, proxies for all video in new
      // projects. The engine has the machine values at once.
      settings = await openSettingsPage(page, app, "Cache and storage");
      await settings
        .getByRole("combobox", { name: "Limit for Proxies" })
        .selectOption(String(100 * GIB));
      await expect
        .poll(async () => proxyLimit(await editorCall<CacheStatus>(page, "media.cache_status")))
        .toBe(100 * GIB);
      await settings.getByRole("tab", { name: "Performance and hardware" }).click();
      await settings.getByRole("combobox", { name: "Proxy jobs at once" }).selectOption("1");
      await expect
        .poll(async () => (await editorCall<CacheStatus>(page, "media.cache_status")).proxy_jobs)
        .toBe(1);
      await settings.getByRole("tab", { name: "Media and import" }).click();
      const policyAgain = settings.getByRole("radiogroup", { name: "Proxies for new projects" });
      await radio(policyAgain, "All video").click();
      await expect(radio(policyAgain, "All video")).toHaveAttribute("aria-checked", "true");
      await snapshot(page, info, "media and import");
      await closeSettings(settings);
      // The import is saved (autosave) before the app is quit, so Exit has nothing to ask.
      await expect(saveStatus(page)).toHaveText("All changes saved");

      const closed = app.waitForEvent("close");
      await chooseMenu(page, app, "File", "Exit");
      await closed;
      await first.close();

      // Relaunch on the same profile: the pages show the choices and the new engine has them.
      const second = await launchFlowApp(work, home, { realMedia: true });
      try {
        const page2 = second.page;
        await expect
          .poll(async () => (await editorCall<CacheStatus>(page2, "media.cache_status")).proxy_jobs)
          .toBe(1);
        const status = await editorCall<CacheStatus>(page2, "media.cache_status");
        expect(proxyLimit(status)).toBe(100 * GIB);
        expect(status.proxy_size).toBe(960);
        settings = await openSettingsPage(page2, second.app, "Media and import");
        await expect(
          radio(
            settings.getByRole("radiogroup", { name: "Proxies for new projects" }),
            "All video",
          ),
        ).toHaveAttribute("aria-checked", "true");
        await settings.getByRole("tab", { name: "Cache and storage" }).click();
        await expect(settings.getByRole("combobox", { name: "Limit for Proxies" })).toHaveValue(
          String(100 * GIB),
        );
        await settings.getByRole("tab", { name: "Performance and hardware" }).click();
        await expect(settings.getByRole("combobox", { name: "Proxy jobs at once" })).toHaveValue(
          "1",
        );
        await closeSettings(settings);

        // The project opens with its proxy still there, and it keeps the policy it was made
        // with; a new project starts with the one chosen in Settings.
        await openRecent(page2, "Proxies.cutweave");
        await expect(mediaCards(page2)).toHaveCount(1);
        await expect(badge(page2)).toHaveAttribute("data-proxy", "ready", { timeout: 60_000 });
        expect((await editorCall<Proxies>(page2, "media.proxy_status", {})).policy).toBe("auto");
        await createProject(second.app, page2, "Second", work.dir);
        await expect
          .poll(async () => (await editorCall<Proxies>(page2, "media.proxy_status", {})).policy)
          .toBe("all");
      } finally {
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
