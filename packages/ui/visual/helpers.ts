// SPDX-License-Identifier: GPL-3.0-or-later
import { expect, type Page } from "@playwright/test";
import type { Theme } from "./shared.ts";

/**
 * `a11y.manual:!true` stops the Storybook a11y addon from running its own axe pass after every
 * render. It shares the page's `axe` global, so it collides with the @axe-core/playwright run
 * ("Axe is already running"); the a11y spec is the single, awaited axe run per page.
 */
const globalsFor = (theme: Theme) => `theme:${theme};a11y.manual:!true`;

/** Opens one story in the isolated iframe, in the given theme, and waits until it is settled. */
export async function openStory(page: Page, id: string, theme: Theme): Promise<void> {
  await page.goto(
    `/iframe.html?id=${encodeURIComponent(id)}&viewMode=story&globals=${globalsFor(theme)}`,
  );
  await page.waitForSelector("body.sb-show-main", { timeout: 30_000 });
  await expect(page.locator("body.sb-show-errordisplay")).toHaveCount(0);
  await page.evaluate(() => document.fonts.ready.then(() => undefined));
  await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
}
