// SPDX-License-Identifier: GPL-3.0-or-later
import { expect, test } from "./fixtures.ts";

test("app starts and renders the editor layout from app://", async ({ page }) => {
  expect(page.url()).toMatch(/^app:\/\/cutweave\//);
  await expect(page.locator(".cw-topbar")).toBeVisible();
  await expect(page.locator(".cw-dock")).toBeVisible();
  // "Cutweave editor" until the engine reports its project, then "<project> — Cutweave"; which one
  // the check sees depends on how fast the engine connects.
  await expect(page).toHaveTitle(/^Cutweave editor$| — Cutweave$/);
});

test("the page main world has no Node access, only window.cutweave", async ({ page }) => {
  await expect(page.locator(".cw-app")).toBeVisible();
  const globals = await page.evaluate(() => ({
    require: typeof (window as unknown as { require?: unknown }).require,
    process: typeof (window as unknown as { process?: unknown }).process,
    module: typeof (window as unknown as { module?: unknown }).module,
    buffer: typeof (window as unknown as { Buffer?: unknown }).Buffer,
    ipcRenderer: typeof (window as unknown as { ipcRenderer?: unknown }).ipcRenderer,
    cutweave: Object.keys(window.cutweave).sort(),
  }));
  expect(globals).toEqual({
    require: "undefined",
    process: "undefined",
    module: "undefined",
    buffer: "undefined",
    ipcRenderer: "undefined",
    cutweave: [
      "agents",
      "app",
      "dialog",
      "edit",
      "engine",
      "files",
      "menu",
      "notify",
      "prefs",
      "preview",
      "recent",
      "shell",
      "window",
    ],
  });
});

test("the fake engine becomes ready and the bridge reports a connection", async ({ page }) => {
  await expect
    .poll(() => page.evaluate(async () => (await window.cutweave.engine.getStatus()).state))
    .toBe("ready");
  const connection = await page.evaluate(() => window.cutweave.engine.getConnection());
  expect(connection?.wsUrl).toBe(`ws://127.0.0.1:${connection?.port}/rpc`);
  expect(connection?.token).toMatch(/^[0-9a-f]{64}$/);
});

test("the preview bridge addon is loaded and rejects a ring that does not exist", async ({
  page,
}) => {
  expect(await page.evaluate(() => window.cutweave.preview.isAvailable())).toBe(true);
  const result = await page.evaluate(() => {
    try {
      window.cutweave.preview.open("cwp-00000001-0000000000000001", "1");
      return "opened";
    } catch (err) {
      return String(err);
    }
  });
  expect(result).toContain("ERR_RING_NOT_FOUND");
});
