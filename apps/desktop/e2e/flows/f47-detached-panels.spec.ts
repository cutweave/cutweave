// SPDX-License-Identifier: GPL-3.0-or-later
// F47 Detached panels (docs/user-flows.md, WP-2H.14, ADR-0031): move the Inspector into its own OS
// window from its tab's menu, edit a clip there, switch the theme, close that window to dock it
// back, move it out again and relaunch (the window comes back), then Reset layout. The engine's
// focus record (ADR-0021) does not change while the focus moves between the editor and the
// detached window. Playwright drives each window as its own page.
import { readFileSync } from "node:fs";
import path from "node:path";
import type { ElectronApplication, Locator, Page } from "@playwright/test";
import { expect, quitApp, test } from "../fixtures.ts";
import {
  chooseMenu,
  chooseTheme,
  clipAt,
  clipBox,
  createProject,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  importWithButton,
  inspector,
  item,
  launchFlowApp,
  layout,
  profileDir,
  removeDir,
  snapshot,
  timelineGeometry,
  workspace,
} from "./helpers.ts";

/** A panel's content area (a dockview tab panel), in the window `page` shows. */
const panel = (page: Page, name: string): Locator =>
  page.getByRole("tabpanel", { name, exact: true });

/** A panel's box once the dock has stopped resizing it (two reads in a row agree). */
async function settledBox(page: Page, name: string) {
  let last = "";
  let found: { x: number; y: number; width: number; height: number } | null = null;
  await expect
    .poll(async () => {
      found = await panel(page, name).boundingBox();
      const now = found === null ? "" : `${found.x}|${found.y}|${found.width}|${found.height}`;
      const same = now !== "" && now === last;
      last = now;
      return same;
    })
    .toBe(true);
  if (found === null) throw new Error(`the ${name} panel is not visible`);
  return found as { x: number; y: number; width: number; height: number };
}

/** The detached windows: every page of the app but the editor's. */
const detachedWindows = (app: ElectronApplication): Page[] =>
  app.windows().filter((w) => w.url().endsWith("/popout.html"));

/** Right-click the panel's tab, choose Move to new window: the window that opens. */
async function moveToNewWindow(app: ElectronApplication, page: Page, name: string): Promise<Page> {
  const opened = app.waitForEvent("window");
  await page.getByRole("tab", { name, exact: true }).click({ button: "right" });
  const menu = page.getByRole("menu", { name: `Panel actions for ${name}` });
  await expect(menu).toBeVisible();
  await menu.getByRole("menuitem", { name: /^Move to new window/ }).click();
  const popout = await opened;
  await popout.waitForLoadState("domcontentloaded");
  return popout;
}

/** The engine's run-file record of when its window last had the focus (ms since the epoch). */
async function focusedAt(app: ElectronApplication, home: string): Promise<number | null> {
  const pid = await app.evaluate(() => globalThis.__cutweaveTest?.enginePid() ?? null);
  if (pid === null) return null;
  try {
    const file = path.join(home, ".cutweave", "run", `${pid}.json`);
    const record = JSON.parse(readFileSync(file, "utf8")) as { focused_at?: number };
    return record.focused_at ?? null;
  } catch {
    // Being rewritten (the engine writes a temp file and renames it): read it again.
    return null;
  }
}

/** The URL of the window the OS has focused, as main sees it. */
const focusedWindow = (app: ElectronApplication) =>
  app.evaluate(({ BrowserWindow }) => BrowserWindow.getFocusedWindow()?.webContents.getURL() ?? "");

/** The active workspace's saved layout has this many detached windows (the editor stores it). */
const storedPopouts = (page: Page) =>
  page.evaluate(() => {
    const view = JSON.parse(localStorage.getItem("cutweave.view") ?? "{}") as {
      state?: { workspaceId?: string; layouts?: Record<string, { popoutGroups?: unknown[] }> };
    };
    return view.state?.layouts?.[view.state.workspaceId ?? ""]?.popoutGroups?.length ?? 0;
  });

test.describe("F47 detached panels", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");
  // macOS keeps the native application menu, which Playwright cannot open (Reset layout there is
  // the native menu's job, as in F33).
  test.skip(process.platform === "darwin", "the page's menu bar is not shown on macOS");

  test("a panel moves into its own window, works there, docks back and is restored on launch", async () => {
    test.setTimeout(240_000);
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    try {
      const first = await launchFlowApp(work, home);
      const { app, page } = first;
      await createProject(app, page, "Detached", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
      await editorCaughtUp(page);

      // 1. Right-click the Inspector's tab, Move to new window: a window of its own, named for
      //    it, shows the Inspector; the main window's Preview takes its room.
      const previewBefore = (await settledBox(page, "Preview")).width;
      const inspectorBefore = await settledBox(page, "Inspector");
      let popout = await moveToNewWindow(app, page, "Inspector");
      await expect(popout).toHaveTitle("Inspector — Detached — Cutweave");
      await expect(panel(popout, "Inspector")).toBeVisible();
      await expect(panel(page, "Inspector")).toHaveCount(0);
      await expect
        .poll(async () => (await settledBox(page, "Preview")).width)
        .toBeGreaterThan(previewBefore);
      expect(detachedWindows(app)).toHaveLength(1);
      await snapshot(popout, info, "inspector in its own window");

      // 2. Click the clip in the main window's timeline: the detached Inspector shows it. Its
      //    opacity, dragged there, reaches the engine.
      const id = clipAt(await timelineGeometry(page), "V1", 1).id;
      const cb = clipBox(await timelineGeometry(page), id);
      await page.mouse.click(cb.x + Math.min(cb.w / 2, 60), cb.y);
      const f = inspector(popout);
      await expect(f.panel).toBeVisible();
      await expect(f.duration).toHaveValue("00:00:10:00");
      const rail = f.panel.locator(".cw-slider__track");
      const box = await rail.boundingBox();
      const thumb = await f.opacity.boundingBox();
      if (box === null || thumb === null) throw new Error("no opacity slider");
      await popout.mouse.move(thumb.x + thumb.width / 2, thumb.y + thumb.height / 2);
      await popout.mouse.down();
      await popout.mouse.move(box.x + box.width / 2, box.y + box.height / 2, { steps: 10 });
      await popout.mouse.up();
      await expect.poll(async () => (await item(page, id)).visual.opacity).toBeLessThan(0.65);
      const dragged = Math.round((await item(page, id)).visual.opacity * 100);
      await expect(f.opacity).toHaveAttribute("aria-valuetext", `${dragged}%`);

      // The focus moving between the editor and its detached window is one window's focus to
      // the engine (ADR-0021): its record of the last focus does not change.
      await page.bringToFront();
      await expect.poll(() => focusedWindow(app)).toBe("app://cutweave/");
      await expect.poll(() => focusedAt(app, home)).not.toBeNull();
      const record = await focusedAt(app, home);
      await popout.bringToFront();
      await expect.poll(() => focusedWindow(app)).toBe("app://cutweave/popout.html");
      await page.bringToFront();
      await expect.poll(() => focusedWindow(app)).toBe("app://cutweave/");
      const reads: (number | null)[] = [];
      await expect
        .poll(
          async () => {
            reads.push(await focusedAt(app, home));
            return reads.slice(-4);
          },
          { intervals: [150] },
        )
        .toEqual([record, record, record, record]);

      // 3. The light theme: the detached window follows.
      await chooseTheme(page, app, "Light");
      await expect(popout.locator("html")).toHaveAttribute("data-theme", "light");
      const background = (p: Page) =>
        p.evaluate(() => getComputedStyle(document.body).backgroundColor);
      await expect.poll(() => background(popout)).toBe(await background(page));
      await snapshot(popout, info, "detached window in the light theme");

      // 4. Close the detached window (the OS close request its close button sends): the
      //    Inspector is docked where it was, right of the Preview at its width.
      const closed = popout.waitForEvent("close");
      await app.evaluate(({ BrowserWindow }) => {
        const win = BrowserWindow.getAllWindows().find((w) =>
          w.webContents.getURL().endsWith("/popout.html"),
        );
        if (win === undefined) throw new Error("no detached window");
        win.close();
      });
      await closed;
      await expect(panel(page, "Inspector")).toBeVisible();
      const inspectorAfter = await settledBox(page, "Inspector");
      const preview = await settledBox(page, "Preview");
      expect(inspectorAfter.x).toBeGreaterThan(preview.x + preview.width - 1);
      expect(Math.abs(inspectorAfter.width - inspectorBefore.width)).toBeLessThan(8);
      expect(detachedWindows(app)).toHaveLength(0);
      await expect.poll(() => storedPopouts(page)).toBe(0);

      // 5. Move it out again and relaunch: the window comes back with the Inspector.
      popout = await moveToNewWindow(app, page, "Inspector");
      await expect(panel(popout, "Inspector")).toBeVisible();
      await expect.poll(() => storedPopouts(page)).toBe(1);
      await quitApp(app, page, "save", first.mainLog);
      await first.close();

      const second = await launchFlowApp(work, home);
      try {
        const page2 = second.page;
        await expect.poll(() => detachedWindows(second.app).length).toBe(1);
        const reopened = detachedWindows(second.app)[0] as Page;
        await expect(panel(reopened, "Inspector")).toBeVisible();
        await expect(reopened).toHaveTitle(/^Inspector — .*Cutweave$/);
        await expect(reopened.locator("html")).toHaveAttribute("data-theme", "light");
        await expect(panel(page2, "Inspector")).toHaveCount(0);
        await expect(panel(page2, "Timeline")).toBeVisible();
        await snapshot(page2, info, "after relaunch, main window");
        await snapshot(reopened, info, "after relaunch, detached window");

        // 6. View > Reset layout: the window closes and the Inspector docks back.
        await chooseMenu(page2, second.app, "View", "Reset layout");
        await expect(panel(page2, "Inspector")).toBeVisible();
        await expect.poll(() => detachedWindows(second.app).length).toBe(0);
        await expect.poll(() => storedPopouts(page2)).toBe(0);
      } finally {
        // 7. Quitting leaves no window behind (quitApp waits for the app to exit).
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
