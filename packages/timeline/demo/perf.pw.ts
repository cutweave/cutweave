// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2A.09 acceptance (arch §19): the 5,000-clip / 50,000-keyframe / 200-media demo pans and
// zooms at >= 55 fps, and hit-tests in p95 < 0.2 ms. The demo's own probe measures; this test
// runs its scripted sweep and prints the numbers. Retried (config) on a loaded machine.
import { expect, test } from "@playwright/test";
import type { DemoResult } from "./main.ts";

const SWEEP_S = 10;

test("pans and zooms the 5,000-clip project at >= 55 fps", async ({ page }) => {
  page.on("pageerror", (e) => console.error("pageerror:", e.message));
  await page.goto(`/?sweep=${SWEEP_S}&warmup=3`);
  await page.waitForFunction(() => window.__demoResult !== undefined || window.__demoError, null, {
    timeout: (SWEEP_S + 60) * 1000,
  });
  const error = await page.evaluate(() => window.__demoError);
  expect(error).toBeUndefined();
  const r = (await page.evaluate(() => window.__demoResult)) as DemoResult;
  console.info(`timeline demo sweep: ${JSON.stringify(r)}`);

  expect(r.dataset).toEqual({ clips: 5000, keyframes: 50_000, media: 200 });
  expect(r.frames).toBeGreaterThan(SWEEP_S * 30);
  // The view drew while it moved (filmstrips included), and only what was visible.
  expect(r.visibleItemsMax).toBeGreaterThan(0);
  expect(r.visibleItemsMax).toBeLessThan(5000);
  expect(r.tilesMax).toBeGreaterThan(0);
  expect(r.fps.mean).toBeGreaterThanOrEqual(55);
  expect(r.hitTestMs.p95).toBeLessThan(0.2);
});
