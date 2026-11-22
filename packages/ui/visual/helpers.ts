// SPDX-License-Identifier: GPL-3.0-or-later
import { expect, type Page } from "@playwright/test";
import type { Theme } from "./shared.ts";

/** Opens one story in the isolated iframe, in the given theme, and waits until it is settled. */
export async function openStory(page: Page, id: string, theme: Theme): Promise<void> {
  await page.goto(
    `/iframe.html?id=${encodeURIComponent(id)}&viewMode=story&globals=theme:${theme}`,
  );
  await page.waitForSelector("body.sb-show-main", { timeout: 30_000 });
  await expect(page.locator("body.sb-show-errordisplay")).toHaveCount(0);
  await page.evaluate(() => document.fonts.ready.then(() => undefined));
  await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
}
