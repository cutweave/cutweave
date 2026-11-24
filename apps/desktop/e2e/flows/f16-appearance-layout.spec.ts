// SPDX-License-Identifier: GPL-3.0-or-later
// F16 Appearance and layout (docs/user-flows.md): Dark/Light, Short-form/Long-form, resize a
// panel by dragging its edge, relaunch. Each choice applies at once and is still there after a
// relaunch on the same profile.
import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import {
  chooseMenu,
  chooseTheme,
  drag,
  hasRealEngine,
  launchFlowApp,
  profileDir,
  removeDir,
  snapshot,
  workspace,
} from "./helpers.ts";

const theme = (page: Page) => page.evaluate(() => document.documentElement.dataset.theme);
const background = (page: Page) =>
  page.evaluate(() => getComputedStyle(document.body).backgroundColor);
/** The Assets panel's content area (a dockview tab panel). */
const assets = (page: Page) => page.getByRole("tabpanel", { name: "Assets", exact: true });
async function width(page: Page): Promise<number> {
  const box = await assets(page).boundingBox();
  if (box === null) throw new Error("the Assets panel is not visible");
  return box.width;
}

/**
 * Whether the editor has stored the Assets column at the width it shows now (read-only: the
 * view settings in the page's storage, written a moment after a resize ends). Quitting before
 * that would race the save, which a person never does.
 */
const layoutStored = (page: Page) =>
  page.evaluate(() => {
    const panel = [...document.querySelectorAll('[role="tabpanel"][aria-labelledby]')].find(
      (el) =>
        document.getElementById(el.getAttribute("aria-labelledby") ?? "")?.textContent?.trim() ===
        "Assets",
    );
    const group = panel?.closest(".dv-groupview");
    const shown = group?.getBoundingClientRect().width ?? Number.NaN;
    const view = JSON.parse(localStorage.getItem("cutweave.view") ?? "{}") as {
      state?: { workspaceId?: string; layouts?: Record<string, { grid?: { root?: unknown } }> };
    };
    const layout = view.state?.layouts?.[view.state.workspaceId ?? ""];
    let stored = Number.NaN;
    const walk = (node: unknown): void => {
      const n = node as { type?: string; data?: unknown; size?: number };
      if (n?.type === "leaf") {
        const views = (n.data as { views?: string[] } | undefined)?.views ?? [];
        if (views.includes("assets") && typeof n.size === "number") stored = n.size;
      } else if (Array.isArray(n?.data)) {
        for (const child of n.data) walk(child);
      }
    };
    walk(layout?.grid?.root);
    return Math.abs(stored - shown) < 1;
  });

test.describe("F16 appearance and layout", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("theme, workspace and a resized panel apply at once and survive a relaunch", async () => {
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    try {
      const first = await launchFlowApp(work, home);
      const { app, page } = first;
      const workspaces = page.getByRole("radiogroup", { name: "Workspace" });
      const aspects = page.getByRole("radiogroup", { name: "Preview aspect ratio" });

      // Dark by default; Light (File > Settings…) applies at once. The title bar has no theme
      // switch.
      expect(await theme(page)).toBe("dark");
      await expect(page.getByRole("radiogroup", { name: "Theme" })).toHaveCount(0);
      const dark = await background(page);
      await chooseTheme(page, app, "Light");
      await expect.poll(() => theme(page)).toBe("light");
      expect(await background(page)).not.toBe(dark);

      // Long-form: its layout, with a 16:9 preview.
      await expect(aspects.getByRole("radio", { name: "9:16" })).toHaveAttribute(
        "aria-checked",
        "true",
      );
      const shortWidth = await width(page);
      await workspaces.getByRole("radio", { name: "Long-form" }).click();
      await expect(aspects.getByRole("radio", { name: "16:9" })).toHaveAttribute(
        "aria-checked",
        "true",
      );
      await expect.poll(() => width(page)).not.toBe(shortWidth);

      // Drag the edge between Assets and Preview 120 px to the right.
      const before = await width(page);
      const box = await assets(page).boundingBox();
      if (box === null) throw new Error("no Assets panel");
      const edge = { x: box.x + box.width + 2, y: box.y + box.height / 2 };
      await drag(page, edge, { x: edge.x + 120, y: edge.y });
      await expect.poll(() => width(page)).toBeGreaterThan(before + 100);
      const resized = await width(page);
      await expect.poll(() => layoutStored(page)).toBe(true);
      await snapshot(page, info, "light, long-form, resized");

      // Relaunch on the same profile.
      const closed = app.waitForEvent("close");
      await chooseMenu(page, app, "File", "Exit");
      await closed;
      await first.close();
      const second = await launchFlowApp(work, home);
      try {
        const page2 = second.page;
        await expect.poll(() => theme(page2)).toBe("light");
        await expect(
          page2.getByRole("radiogroup", { name: "Workspace" }).getByRole("radio", {
            name: "Long-form",
          }),
        ).toHaveAttribute("aria-checked", "true");
        await expect(
          page2.getByRole("radiogroup", { name: "Preview aspect ratio" }).getByRole("radio", {
            name: "16:9",
          }),
        ).toHaveAttribute("aria-checked", "true");
        await expect.poll(async () => Math.abs((await width(page2)) - resized)).toBeLessThan(3);
        await snapshot(page2, info, "after relaunch");

        // Back to Dark and Short-form, as a person tidying up would.
        await chooseTheme(page2, second.app, "Dark");
        await expect.poll(() => theme(page2)).toBe("dark");
        await page2
          .getByRole("radiogroup", { name: "Workspace" })
          .getByRole("radio", { name: "Short-form" })
          .click();
        await expect(
          page2.getByRole("radiogroup", { name: "Preview aspect ratio" }).getByRole("radio", {
            name: "9:16",
          }),
        ).toHaveAttribute("aria-checked", "true");
      } finally {
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
