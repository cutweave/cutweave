// SPDX-License-Identifier: GPL-3.0-or-later
// Density and interface scale (WP-2H.09): the editor shell and the timeline canvas in both
// densities and both themes, at 100% and at 125%, compared to their own baselines, and axe on
// both in the Comfortable density (a11y.spec.ts covers Standard).
//
// The interface scale is the window's zoom factor. Chromium lays a zoomed page out in fewer CSS
// pixels and raises the device pixel ratio by the same factor, so 125% here is the same window
// (1280 × 800 device pixels) with a 1024 × 640 viewport at a device scale factor of 1.25, and the
// screenshot is taken in device pixels. Standard at 100% is the story's ordinary baseline
// (visual.spec.ts); the other three are `<story-id>@<density>-<scale>.png` beside it.
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import AxeBuilder from "@axe-core/playwright";
import { type Browser, expect, type Page, test } from "@playwright/test";
import { compareImages } from "./compare.ts";
import {
  ACTUAL_DIR,
  BASELINE_DIR,
  hasAnyBaseline,
  PSNR_MIN,
  SSIM_MIN,
  THEMES,
  type Theme,
  VIEWPORT,
} from "./shared.ts";

const UPDATE = process.env.UI_SNAPSHOTS_UPDATE === "1";
const BOOTSTRAP = !UPDATE && !hasAnyBaseline();

const STORIES = ["foundations-editor-shell--at-rest", "timeline-timelinecanvas--mixed-tracks"];
const DENSITIES = ["compact", "comfortable"] as const;
type Density = (typeof DENSITIES)[number];
const SCALES = [100, 125] as const;
type Scale = (typeof SCALES)[number];

/** What the density is called in Settings and in the baseline's name. */
const densityName = (density: Density): string =>
  density === "compact" ? "standard" : "comfortable";

/** Control heights (`--control-sm`, `-md`, `-lg`) and the top bar, in CSS pixels, per density. */
const SIZES: Record<Density, readonly number[]> = {
  compact: [24, 28, 32, 40],
  comfortable: [28, 32, 36, 44],
};

function write(path: string, data: Buffer) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, data);
}

/** A page whose window is zoomed to `scale`: fewer CSS pixels, each one more device pixels. */
async function pageAt(browser: Browser, scale: Scale): Promise<Page> {
  const factor = scale / 100;
  const context = await browser.newContext({
    viewport: {
      width: Math.round(VIEWPORT.width / factor),
      height: Math.round(VIEWPORT.height / factor),
    },
    deviceScaleFactor: factor,
  });
  return context.newPage();
}

async function openStory(page: Page, id: string, theme: Theme, density: Density): Promise<void> {
  await page.goto(
    `/iframe.html?id=${encodeURIComponent(id)}&viewMode=story` +
      `&globals=theme:${theme};density:${density};a11y.manual:!true`,
  );
  await page.waitForSelector("body.sb-show-main", { timeout: 30_000 });
  await expect(page.locator("body.sb-show-errordisplay")).toHaveCount(0);
  await page.evaluate(() => document.fonts.ready.then(() => undefined));
  await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
  await expect(page.locator("html")).toHaveAttribute("data-density", density);
}

for (const story of STORIES) {
  for (const theme of THEMES) {
    for (const density of DENSITIES) {
      for (const scale of SCALES) {
        // Standard at 100% is the story's own baseline (visual.spec.ts).
        if (density === "compact" && scale === 100) continue;
        const name = `${story}@${densityName(density)}-${scale}`;
        test(`${name} [${theme}]`, async ({ browser }, testInfo) => {
          const page = await pageAt(browser, scale);
          try {
            await openStory(page, story, theme, density);
            // The size tokens are the density's, whatever the scale.
            const sizes = await page.evaluate(() => {
              const cs = getComputedStyle(document.documentElement);
              return ["--control-sm", "--control-md", "--control-lg", "--toolbar"].map((token) =>
                Number.parseFloat(cs.getPropertyValue(token)),
              );
            });
            expect(sizes).toEqual(SIZES[density]);
            expect(await page.evaluate(() => devicePixelRatio)).toBeCloseTo(scale / 100, 3);

            const actual = await page.screenshot({
              fullPage: true,
              animations: "disabled",
              caret: "hide",
              scale: "device",
            });
            const rel = `${theme}/${name}.png`;
            const baseline = resolve(BASELINE_DIR, rel);
            if (!existsSync(baseline)) {
              if (UPDATE || BOOTSTRAP) {
                write(baseline, actual);
                testInfo.annotations.push({ type: "baseline-written", description: rel });
                return;
              }
              write(resolve(ACTUAL_DIR, rel), actual);
              throw new Error(
                `No baseline for ${rel}. Run \`just ui-snapshots --update\` and commit the new PNG.`,
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
            if (result.diff) write(resolve(ACTUAL_DIR, `${theme}/${name}.diff.png`), result.diff);
            await testInfo.attach("actual", { body: actual, contentType: "image/png" });
            expect(
              result,
              `${rel}: ${result.sizeMatch ? "" : "size changed; "}SSIM ${result.ssim.toFixed(5)} ` +
                `(min ${SSIM_MIN}), PSNR ${result.psnr.toFixed(2)} dB (min ${PSNR_MIN}).`,
            ).toMatchObject({ pass: true });
          } finally {
            await page.context().close();
          }
        });
      }
    }

    test(`axe: ${story} in Comfortable [${theme}]`, async ({ browser }) => {
      const page = await pageAt(browser, 100);
      try {
        await openStory(page, story, theme, "comfortable");
        const { violations } = await new AxeBuilder({ page })
          .include("#storybook-root")
          .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa", "best-practice"])
          .analyze();
        const found = violations.map(
          (v) => `${v.id} (${v.impact}): ${v.help}\n    ${v.nodes.map((n) => n.target).join(" ")}`,
        );
        expect(found, found.join("\n")).toEqual([]);
      } finally {
        await page.context().close();
      }
    });
  }
}

// Every component in the Comfortable density: no axe violation in any story (dark theme; the
// colors, and so the contrast, are the same in both densities and are checked in a11y.spec.ts).
test("axe: every story in Comfortable has no violation", async ({ browser }) => {
  test.setTimeout(600_000);
  const { listStories } = await import("./shared.ts");
  const page = await pageAt(browser, 100);
  const found: string[] = [];
  try {
    for (const story of listStories()) {
      await openStory(page, story.id, "dark", "comfortable");
      const overlays = "body > [data-radix-popper-content-wrapper], body > [data-radix-portal]";
      const hasOverlay = (await page.locator(overlays).count()) > 0;
      const { violations } = await new AxeBuilder({ page })
        .include(hasOverlay ? ["#storybook-root", overlays] : ["#storybook-root"])
        .withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa", "best-practice"])
        // A story that opens a modal layer on load hides its root from assistive tech while the
        // focus is trapped in the layer: a11y.spec.ts explains and checks that case.
        .disableRules(hasOverlay ? ["aria-hidden-focus"] : [])
        .analyze();
      for (const v of violations) {
        found.push(`${story.id}: ${v.id} (${v.impact}) ${v.nodes.map((n) => n.target).join(" ")}`);
      }
    }
  } finally {
    await page.context().close();
  }
  expect(found, found.join("\n")).toEqual([]);
});
