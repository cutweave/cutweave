// SPDX-License-Identifier: GPL-3.0-or-later
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { expect, test } from "@playwright/test";
import { compareImages } from "./compare.ts";
import { openStory } from "./helpers.ts";
import {
  ACTUAL_DIR,
  BASELINE_DIR,
  hasAnyBaseline,
  listStories,
  PSNR_MIN,
  SSIM_MIN,
  THEMES,
} from "./shared.ts";

// Baselines are Linux software-rendered Chromium. Other platforms render differently, so the
// comparison runs only on Linux (override with UI_SNAPSHOTS_ANY_PLATFORM=1 for debugging).
test.skip(
  process.platform !== "linux" && process.env.UI_SNAPSHOTS_ANY_PLATFORM !== "1",
  "visual baselines are Linux-only (software-rendered Chromium); CI produces and checks them",
);

const UPDATE = process.env.UI_SNAPSHOTS_UPDATE === "1";
// First run on a checkout with no committed baselines: write them (CI uploads them as an
// artifact to commit) instead of failing. Once any baseline exists, a missing one fails.
const BOOTSTRAP = !UPDATE && !hasAnyBaseline();

function write(path: string, data: Buffer) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, data);
}

for (const story of listStories()) {
  for (const theme of THEMES) {
    test(`${story.id} [${theme}]`, async ({ page }, testInfo) => {
      await openStory(page, story.id, theme);
      const actual = await page.screenshot({
        fullPage: true,
        animations: "disabled",
        caret: "hide",
        scale: "css",
      });
      const rel = `${theme}/${story.id}.png`;
      const baseline = resolve(BASELINE_DIR, rel);

      if (!existsSync(baseline)) {
        if (UPDATE || BOOTSTRAP) {
          write(baseline, actual);
          testInfo.annotations.push({ type: "baseline-written", description: rel });
          return;
        }
        write(resolve(ACTUAL_DIR, rel), actual);
        throw new Error(
          `No baseline for ${rel}. Add the \`ui-snapshots-update\` label to the PR (or run ` +
            "`just ui-snapshots --update` on Linux) and commit the new PNGs.",
        );
      }

      const result = compareImages(readFileSync(baseline), actual);
      if (result.pass) return;
      if (UPDATE) {
        write(baseline, actual);
        testInfo.annotations.push({ type: "baseline-updated", description: rel });
        return;
      }
      write(resolve(ACTUAL_DIR, rel), actual);
      if (result.diff) write(resolve(ACTUAL_DIR, `${theme}/${story.id}.diff.png`), result.diff);
      await testInfo.attach("actual", { body: actual, contentType: "image/png" });
      expect(
        result,
        `${rel}: ${result.sizeMatch ? "" : "size changed; "}SSIM ${result.ssim.toFixed(5)} ` +
          `(min ${SSIM_MIN}), PSNR ${result.psnr.toFixed(2)} dB (min ${PSNR_MIN}). Actual and ` +
          "diff are in __screenshots-actual__/. If the change is intended, apply the " +
          "`ui-snapshots-update` PR label.",
      ).toMatchObject({ pass: true });
    });
  }
}
