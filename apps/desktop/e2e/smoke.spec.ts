// SPDX-License-Identifier: GPL-3.0-or-later
import { expect, test } from "./fixtures.ts";

test("app starts and renders the editor layout from app://", async ({ page }) => {
  expect(page.url()).toMatch(/^app:\/\/cutweave\//);
  await expect(page.locator(".cw-topbar")).toBeVisible();
  await expect(page.locator(".cw-dock")).toBeVisible();
  await expect(page).toHaveTitle("Cutweave editor");
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
    cutweave: ["app", "dialog", "engine", "menu", "preview", "shell"],
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

test("the preview bridge is a typed stub until the addon lands", async ({ page }) => {
  const result = await page.evaluate(() => {
    try {
      window.cutweave.preview.open("ring");
      return "opened";
    } catch (err) {
      return String(err);
    }
  });
  expect(result).toContain("ERR_BRIDGE_UNAVAILABLE");
});
