// SPDX-License-Identifier: GPL-3.0-or-later
// F33 Close and reopen panels (docs/user-flows.md, WP-2H.01): close the Inspector with its close
// button, reopen it from View > Panels; close Assets in one workspace and switch; close every
// panel and reset the layout; relaunch. Real clicks on the buttons and the real menu bar.
import type { ElectronApplication, Locator, Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import {
  chooseMenu,
  hasRealEngine,
  launchFlowApp,
  profileDir,
  removeDir,
  snapshot,
  workspace,
} from "./helpers.ts";

const NAMES = ["Assets", "Preview", "Inspector", "Timeline"] as const;
type PanelName = (typeof NAMES)[number];

/** A panel's content area (a dockview tab panel). */
const panel = (page: Page, name: PanelName): Locator =>
  page.getByRole("tabpanel", { name, exact: true });
const closeButton = (page: Page, name: PanelName): Locator =>
  page.getByRole("button", { name: `Close ${name} panel`, exact: true });

async function box(page: Page, name: PanelName) {
  const found = await panel(page, name).boundingBox();
  if (found === null) throw new Error(`the ${name} panel is not visible`);
  return found;
}

/** A panel's box once the dock has stopped resizing it (two reads in a row agree). */
async function settledBox(page: Page, name: PanelName) {
  let last = "";
  await expect
    .poll(async () => {
      const b = await box(page, name);
      const now = `${b.x}|${b.y}|${b.width}|${b.height}`;
      const same = now === last;
      last = now;
      return same;
    })
    .toBe(true);
  return box(page, name);
}

/** View > Panels, opened with real clicks; the submenu's list. */
async function openPanelsMenu(page: Page): Promise<Locator> {
  await page
    .getByRole("menubar", { name: "Application menu" })
    .getByRole("menuitem", { name: "View", exact: true })
    .click();
  await page.getByRole("menu", { name: "View" }).getByRole("menuitem", { name: "Panels" }).click();
  const list = page.getByRole("menu", { name: "Panels" });
  await expect(list).toBeVisible();
  return list;
}

/** Choose a panel in View > Panels (it toggles). */
async function togglePanel(page: Page, name: PanelName): Promise<void> {
  const list = await openPanelsMenu(page);
  await list.getByRole("menuitemcheckbox", { name, exact: true }).click();
  await expect(page.getByRole("menu")).toHaveCount(0);
}

/** View > Panels' checkmarks, read the way a person sees them, then the menu is closed. */
async function checks(page: Page): Promise<Record<PanelName, boolean>> {
  const list = await openPanelsMenu(page);
  const out = {} as Record<PanelName, boolean>;
  for (const name of NAMES) {
    out[name] =
      (await list
        .getByRole("menuitemcheckbox", { name, exact: true })
        .getAttribute("aria-checked")) === "true";
  }
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await expect(page.getByRole("menu")).toHaveCount(0);
  return out;
}

/** The saved layout of the active workspace lists these panels as closed (the editor stores it). */
const storedClosed = (page: Page) =>
  page.evaluate(() => {
    const view = JSON.parse(localStorage.getItem("cutweave.view") ?? "{}") as {
      state?: {
        workspaceId?: string;
        layouts?: Record<string, { cutweave?: { closed?: string[] } }>;
      };
    };
    return view.state?.layouts?.[view.state.workspaceId ?? ""]?.cutweave?.closed ?? [];
  });

test.describe("F33 close and reopen panels", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");
  // macOS keeps the native application menu, which Playwright cannot open (View > Panels there
  // is the native menu's job: see the notes of WP-2H.01).
  test.skip(process.platform === "darwin", "the page's menu bar is not shown on macOS");

  test("a panel closes with its button, comes back where it was, and the choice survives a relaunch", async () => {
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    try {
      const first = await launchFlowApp(work, home);
      const { app, page } = first;
      const workspaces = page.getByRole("radiogroup", { name: "Workspace" });
      await expect(panel(page, "Inspector")).toBeVisible();

      // Close the Inspector with its close button: the Preview widens and the menu says so.
      const previewBefore = (await settledBox(page, "Preview")).width;
      const inspectorBefore = await settledBox(page, "Inspector");
      await closeButton(page, "Inspector").click();
      await expect(panel(page, "Inspector")).toHaveCount(0);
      await expect
        .poll(async () => (await box(page, "Preview")).width)
        .toBeGreaterThan(previewBefore);
      expect(await checks(page)).toEqual({
        Assets: true,
        Preview: true,
        Inspector: false,
        Timeline: true,
      });

      // Choosing it in the menu brings it back where it was: right of the Preview, at its width.
      await togglePanel(page, "Inspector");
      await expect(panel(page, "Inspector")).toBeVisible();
      const inspectorAfter = await settledBox(page, "Inspector");
      const preview = await box(page, "Preview");
      expect(inspectorAfter.x).toBeGreaterThan(preview.x + preview.width - 1);
      expect(Math.abs(inspectorAfter.width - inspectorBefore.width)).toBeLessThan(8);
      expect((await checks(page)).Inspector).toBe(true);

      // Close Assets in Short-form; Long-form (its own set) has Assets; Short-form still has it closed.
      await closeButton(page, "Assets").click();
      await expect(panel(page, "Assets")).toHaveCount(0);
      await workspaces.getByRole("radio", { name: "Long-form" }).click();
      await expect(panel(page, "Assets")).toBeVisible();
      await workspaces.getByRole("radio", { name: "Short-form" }).click();
      await expect(panel(page, "Timeline")).toBeVisible();
      await expect(panel(page, "Assets")).toHaveCount(0);
      await togglePanel(page, "Assets");
      await expect(panel(page, "Assets")).toBeVisible();

      // Close every panel: the empty dock offers the way back.
      for (const name of NAMES) {
        await closeButton(page, name).click();
        await expect(panel(page, name)).toHaveCount(0);
      }
      const empty = page.getByRole("main");
      await expect(empty.getByText("Every panel is closed")).toBeVisible();
      await snapshot(page, info, "every panel closed");
      await empty.getByRole("button", { name: "Reset layout" }).click();
      for (const name of NAMES) await expect(panel(page, name)).toBeVisible();
      await expect(empty.getByText("Every panel is closed")).toHaveCount(0);

      // Close the Inspector and quit: it is still closed after the relaunch.
      await closeButton(page, "Inspector").click();
      await expect(panel(page, "Inspector")).toHaveCount(0);
      await expect.poll(() => storedClosed(page)).toEqual(["inspector"]);
      await quit(app, page);
      await first.close();

      const second = await launchFlowApp(work, home);
      try {
        const page2 = second.page;
        await expect(panel(page2, "Timeline")).toBeVisible();
        await expect(panel(page2, "Inspector")).toHaveCount(0);
        expect((await checks(page2)).Inspector).toBe(false);
        await snapshot(page2, info, "after relaunch");

        // View > Reset layout puts every panel back, as a person tidying up would.
        await chooseMenu(page2, second.app, "View", "Reset layout");
        await expect(panel(page2, "Inspector")).toBeVisible();
      } finally {
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});

async function quit(app: ElectronApplication, page: Page): Promise<void> {
  const closed = app.waitForEvent("close");
  await chooseMenu(page, app, "File", "Exit");
  await closed;
}
