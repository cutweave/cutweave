// SPDX-License-Identifier: GPL-3.0-or-later
// F26 Interface scale and density (docs/user-flows.md; WP-2H.09), as a person does it: Settings >
// Appearance: Interface scale 125% and Density Comfortable, and the controls grow at once, in the
// editor window and in a detached panel's window; quit and relaunch, and both are still there;
// Reduce motion On, and dialogs open with no transition; Off keeps the motion even when the
// system asks to reduce it. Settings passes axe in both densities.
//
// The second test is the scale's promise for the two canvases: at 90% and at 125%, dragging a clip
// by 100 CSS px moves it by the same time as at 100%, and the preview canvas's backing store is
// its CSS size × the device pixel ratio.
//
// At another interface scale Playwright still places its pointer in the window's own points, so
// a click by element would land beside it: `hand` asks the page where an element is and converts
// (it measures the conversion once per scale by moving the mouse and reading where the page saw
// it). The keyboard and the OS's reduced-motion setting (`emulateMedia`) need no conversion.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import type { ElectronApplication, Locator, Page } from "@playwright/test";
import { desktopDir, expect, test } from "../fixtures.ts";
import {
  clipAt,
  clipBox,
  createProject,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  layout,
  profileDir,
  removeDir,
  shortcut,
  snapshot,
  timelineGeometry,
  workspace,
} from "./helpers.ts";

declare global {
  /** Where the page last saw the mouse, in its own CSS pixels (set by `hand`). */
  var __cutweaveFlowMouse: { x: number; y: number } | undefined;
}

type Scale = 90 | 100 | 110 | 125 | 150;

/** The zoom factor main gave the editor's window, or its detached panel's window. */
const zoomFactor = (app: ElectronApplication, which: "editor" | "detached" = "editor") =>
  app.evaluate(
    ({ BrowserWindow }, detached) =>
      BrowserWindow.getAllWindows()
        .find((w) => w.webContents.getURL().endsWith("/popout.html") === detached)
        ?.webContents.getZoomFactor() ?? 0,
    which === "detached",
  );

const rootAttribute = (page: Page, name: string) =>
  page.evaluate((attribute) => document.documentElement.getAttribute(attribute), name);

/** An element's box in the page's CSS pixels. */
const cssBox = (target: Locator) =>
  target.evaluate((el) => {
    const r = el.getBoundingClientRect();
    return { x: r.x, y: r.y, width: r.width, height: r.height };
  });

/** A size token of the page, in CSS pixels. */
const token = (page: Page, name: string) =>
  page.evaluate(
    (n) => Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue(n)),
    name,
  );

/**
 * A hand on the mouse at the window's current interface scale: `click`, `rightClick` and `drag`
 * take positions in the page's CSS pixels.
 */
async function hand(page: Page) {
  await page.evaluate(() => {
    globalThis.__cutweaveFlowMouse = undefined;
    window.addEventListener(
      "mousemove",
      (event) => {
        globalThis.__cutweaveFlowMouse = { x: event.clientX, y: event.clientY };
      },
      { capture: true, passive: true },
    );
  });
  // Over the preview's stage: nothing there reacts to a pointer passing by.
  const probe = { x: 400, y: 200 };
  await page.mouse.move(probe.x - 20, probe.y - 20);
  await page.mouse.move(probe.x, probe.y);
  await expect
    .poll(() => page.evaluate(() => globalThis.__cutweaveFlowMouse?.x))
    .toBeGreaterThan(0);
  const seen = await page.evaluate(() => globalThis.__cutweaveFlowMouse ?? { x: 0, y: 0 });
  /** Pointer units per CSS pixel. */
  const k = probe.x / seen.x;
  const at = (x: number, y: number) => ({ x: x * k, y: y * k });
  const centre = async (target: Locator) => {
    await target.scrollIntoViewIfNeeded();
    const box = await cssBox(target);
    return at(box.x + box.width / 2, box.y + box.height / 2);
  };
  return {
    /** Pointer units per CSS pixel: 1 at 100%, the zoom factor at another scale. */
    k,
    async click(target: Locator, button: "left" | "right" = "left"): Promise<void> {
      await expect(target).toBeVisible();
      const p = await centre(target);
      await page.mouse.click(p.x, p.y, { button });
    },
    /** A drag with the left button between two points given in CSS pixels, in small steps. */
    async drag(from: { x: number; y: number }, to: { x: number; y: number }): Promise<void> {
      const a = at(from.x, from.y);
      const b = at(to.x, to.y);
      await page.mouse.move(a.x, a.y);
      await page.mouse.down();
      await page.mouse.move(a.x + Math.sign(b.x - a.x) * 4, a.y, { steps: 2 });
      await page.mouse.move(b.x, b.y, { steps: 12 });
      await page.mouse.up();
    },
  };
}

/** Settings, opened with Ctrl+, on its Appearance page. */
async function openAppearance(app: ElectronApplication, page: Page): Promise<Locator> {
  await shortcut(app, "Control+,");
  const settings = page.getByRole("dialog", { name: "Settings" });
  await expect(settings).toBeVisible();
  const tab = settings.getByRole("tab", { name: "Appearance" });
  await (await hand(page)).click(tab);
  await expect(settings.getByRole("heading", { name: "Appearance", level: 3 })).toBeVisible();
  return settings;
}

const choice = (settings: Locator, group: string, name: string): Locator =>
  settings.getByRole("radiogroup", { name: group }).getByRole("radio", { name, exact: true });

/** Picks a choice of a row of the open Settings dialog and waits until it is the chosen one. */
async function choose(page: Page, settings: Locator, group: string, name: string): Promise<void> {
  const radio = choice(settings, group, name);
  await (await hand(page)).click(radio);
  await expect(radio).toHaveAttribute("aria-checked", "true");
}

/** Closes Settings with Done, at any scale. */
async function done(page: Page, settings: Locator): Promise<void> {
  await (await hand(page)).click(settings.getByRole("button", { name: "Done" }));
  await expect(settings).toBeHidden();
}

/** Settings > Appearance > Interface scale, then Done: resolves once the window has the zoom. */
async function setScale(app: ElectronApplication, page: Page, scale: Scale): Promise<void> {
  const settings = await openAppearance(app, page);
  await choose(page, settings, "Interface scale", `${scale}%`);
  await expect.poll(() => zoomFactor(app)).toBeCloseTo(scale / 100, 3);
  await done(page, settings);
}

/** axe's findings for the open Settings dialog, as `rule: first node`. */
async function settingsViolations(page: Page): Promise<string[]> {
  const source = readFileSync(
    createRequire(path.join(desktopDir, "..", "..", "packages", "editor", "package.json")).resolve(
      "axe-core/axe.min.js",
    ),
    "utf8",
  );
  await page.evaluate(source);
  return page.evaluate(async () => {
    interface AxeResult {
      violations: { id: string; nodes: { html: string; failureSummary?: string }[] }[];
    }
    const axe = (globalThis as unknown as { axe: { run(context: Element): Promise<AxeResult> } })
      .axe;
    const dialog = document.querySelector('[role="dialog"]');
    if (dialog === null) return ["Settings is not open"];
    // The dialog fades in: its colours are the ones the person reads once it has.
    await Promise.all(dialog.getAnimations({ subtree: true }).map((a) => a.finished));
    const results = await axe.run(dialog);
    return results.violations.map(
      (v) => `${v.id}: ${v.nodes[0]?.html ?? ""} ${v.nodes[0]?.failureSummary ?? ""}`,
    );
  });
}

/** How the open Settings dialog came in: the name of its opening animation, or `none`. */
const openingAnimation = (settings: Locator) =>
  settings.evaluate((el) => getComputedStyle(el).animationName);

test.describe("F26 interface scale and density", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("125% and Comfortable apply at once, in a detached panel too, and persist; Reduce motion", async () => {
    test.setTimeout(240_000);
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    try {
      const first = await launchFlowApp(work, home);
      const { app, page } = first;
      await createProject(app, page, "Scale", work.dir);
      const topBar = page.getByRole("banner");
      const dpr = await page.evaluate(() => devicePixelRatio);
      const width = await page.evaluate(() => innerWidth);
      expect(await zoomFactor(app)).toBeCloseTo(1, 3);
      expect(await rootAttribute(page, "data-density")).toBe("compact");
      expect((await cssBox(topBar)).height).toBe(40);

      // Settings > Appearance: the three new rows, at their defaults. Standard passes axe.
      let settings = await openAppearance(app, page);
      await expect(
        settings.getByRole("radiogroup", { name: "Interface scale" }).getByRole("radio"),
      ).toHaveText(["90%", "100%", "110%", "125%", "150%"]);
      await expect(choice(settings, "Interface scale", "100%")).toHaveAttribute(
        "aria-checked",
        "true",
      );
      await expect(choice(settings, "Density", "Standard")).toHaveAttribute("aria-checked", "true");
      await expect(choice(settings, "Reduce motion", "Match system")).toHaveAttribute(
        "aria-checked",
        "true",
      );
      expect(await settingsViolations(page)).toEqual([]);
      const doneButton = settings.getByRole("button", { name: "Done" });
      expect((await cssBox(doneButton)).height).toBe(await token(page, "--control-md"));
      await snapshot(page, info, "appearance standard 100");

      // 125%: at once, with Settings still open. The page has fewer CSS pixels, each one more
      // device pixels, so everything is a quarter larger on screen.
      await choose(page, settings, "Interface scale", "125%");
      await expect.poll(() => zoomFactor(app)).toBeCloseTo(1.25, 3);
      await expect.poll(() => page.evaluate(() => devicePixelRatio)).toBeCloseTo(dpr * 1.25, 2);
      await expect.poll(() => page.evaluate(() => innerWidth)).toBeGreaterThan(width / 1.25 - 2);
      expect(await page.evaluate(() => innerWidth)).toBeLessThan(width / 1.25 + 2);
      expect((await hand(page)).k).toBeCloseTo(1.25, 2);
      // The OS window controls stay as tall as the top bar (Windows and Linux draw them).
      const overlay = () =>
        page.evaluate(() => {
          const controls = (
            navigator as unknown as {
              windowControlsOverlay?: { visible: boolean; getTitlebarAreaRect(): DOMRect };
            }
          ).windowControlsOverlay;
          return controls?.visible === true ? controls.getTitlebarAreaRect().height : null;
        });
      if ((await overlay()) !== null) {
        await expect.poll(async () => Math.abs(((await overlay()) ?? 0) - 40)).toBeLessThan(1);
      }
      await snapshot(page, info, "appearance standard 125");

      // Comfortable: at once too. The top bar and the controls are taller, the text is not.
      await choose(page, settings, "Density", "Comfortable");
      await expect.poll(() => rootAttribute(page, "data-density")).toBe("comfortable");
      await expect.poll(async () => (await cssBox(topBar)).height).toBe(44);
      expect(await token(page, "--control-md")).toBe(32);
      expect((await cssBox(doneButton)).height).toBe(32);
      expect(await token(page, "--track-md")).toBe(56);
      if ((await overlay()) !== null) {
        await expect.poll(async () => Math.abs(((await overlay()) ?? 0) - 44)).toBeLessThan(1);
      }
      expect(await settingsViolations(page)).toEqual([]);
      await snapshot(page, info, "appearance comfortable 125");
      await done(page, settings);
      await snapshot(page, info, "editor comfortable 125");

      // A detached panel's window (F47) has the same scale and density.
      const opened = app.waitForEvent("window");
      const pointer = await hand(page);
      await pointer.click(page.getByRole("tab", { name: "Inspector", exact: true }), "right");
      const menu = page.getByRole("menu", { name: "Panel actions for Inspector" });
      await expect(menu).toBeVisible();
      await pointer.click(menu.getByRole("menuitem", { name: /^Move to new window/ }));
      const popout = await opened;
      await popout.waitForLoadState("domcontentloaded");
      await expect(popout.getByRole("region", { name: "Inspector", exact: true })).toBeVisible();
      await expect.poll(() => zoomFactor(app, "detached")).toBeCloseTo(1.25, 3);
      await expect.poll(() => popout.evaluate(() => devicePixelRatio)).toBeCloseTo(dpr * 1.25, 2);
      await expect.poll(() => rootAttribute(popout, "data-density")).toBe("comfortable");
      expect(await token(popout, "--control-md")).toBe(32);
      // Back to 100% and to 125% again: the detached window follows the editor's, at once.
      await setScale(app, page, 100);
      await expect.poll(() => zoomFactor(app, "detached")).toBeCloseTo(1, 3);
      await setScale(app, page, 125);
      await expect.poll(() => zoomFactor(app, "detached")).toBeCloseTo(1.25, 3);
      await snapshot(popout, info, "detached inspector comfortable 125");
      // Close it as its close button does; the Inspector docks back.
      await app.evaluate(({ BrowserWindow }) => {
        BrowserWindow.getAllWindows()
          .find((w) => w.webContents.getURL().endsWith("/popout.html"))
          ?.close();
      });
      await expect(page.getByRole("region", { name: "Inspector", exact: true })).toBeVisible();

      // Quit and relaunch on the same profile: the window opens at 125%, Comfortable.
      await first.close();
      const second = await launchFlowApp(work, home);
      try {
        const page2 = second.page;
        expect(await zoomFactor(second.app)).toBeCloseTo(1.25, 3);
        expect(await page2.evaluate(() => devicePixelRatio)).toBeCloseTo(dpr * 1.25, 2);
        expect(await rootAttribute(page2, "data-density")).toBe("comfortable");
        expect((await cssBox(page2.getByRole("banner"))).height).toBe(44);
        await snapshot(page2, info, "editor after relaunch");

        // The system does not ask to reduce motion (the driver stands in for the OS setting):
        // with "Match system" Settings fades in.
        await page2.emulateMedia({ reducedMotion: "no-preference" });
        settings = await openAppearance(second.app, page2);
        await expect(choice(settings, "Interface scale", "125%")).toHaveAttribute(
          "aria-checked",
          "true",
        );
        await expect(choice(settings, "Density", "Comfortable")).toHaveAttribute(
          "aria-checked",
          "true",
        );
        expect(await rootAttribute(page2, "data-motion")).toBeNull();
        expect(await openingAnimation(settings)).toBe("cw-fade-in");

        // Reduce motion On: dialogs open with no transition, and nothing else animates either.
        await choose(page2, settings, "Reduce motion", "On");
        await expect.poll(() => rootAttribute(page2, "data-motion")).toBe("reduce");
        await done(page2, settings);
        settings = await openAppearance(second.app, page2);
        expect(await openingAnimation(settings)).toBe("none");
        expect(await settings.evaluate((el) => getComputedStyle(el).opacity)).toBe("1");
        expect(
          await choice(settings, "Reduce motion", "On").evaluate(
            (el) => getComputedStyle(el).transitionProperty,
          ),
        ).toBe("none");

        // The system asks to reduce motion: Off keeps it, and Match system follows the system.
        await page2.emulateMedia({ reducedMotion: "reduce" });
        await choose(page2, settings, "Reduce motion", "Off");
        await expect.poll(() => rootAttribute(page2, "data-motion")).toBe("full");
        await done(page2, settings);
        settings = await openAppearance(second.app, page2);
        expect(await openingAnimation(settings)).toBe("cw-fade-in");
        await choose(page2, settings, "Reduce motion", "Match system");
        await expect.poll(() => rootAttribute(page2, "data-motion")).toBeNull();
        await done(page2, settings);
        settings = await openAppearance(second.app, page2);
        expect(await openingAnimation(settings)).toBe("none");
        await done(page2, settings);
      } finally {
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });

  test("at 90% and 125% a 100 px drag moves a clip as far as at 100%, and the preview stays sharp", async () => {
    test.setTimeout(240_000);
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Drag", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 2);
      const at = (start: number) => [
        { track: "V1", clips: [[start, 10]] },
        { track: "A1", clips: [[start, 10]] },
      ];
      await expect.poll(() => layout(page)).toEqual(at(2));
      await editorCaughtUp(page);
      const dpr = await page.evaluate(() => devicePixelRatio);

      /** Seconds the clip moved for a drag of 100 CSS px to the right at this scale. */
      const moved = async (scale: Scale): Promise<number> => {
        await setScale(app, page, scale);
        await expect
          .poll(() => page.evaluate(() => devicePixelRatio))
          .toBeCloseTo((dpr * scale) / 100, 2);

        // The preview canvas: its backing store is its CSS size × the device pixel ratio.
        await expect
          .poll(
            () =>
              page.evaluate(() => {
                const canvas = document.querySelector<HTMLCanvasElement>(
                  "canvas.cw-preview__canvas",
                );
                if (canvas === null) return "no preview canvas";
                const css = canvas.getBoundingClientRect();
                const ratio = window.devicePixelRatio;
                return Math.abs(canvas.width - css.width * ratio) <= 1 &&
                  Math.abs(canvas.height - css.height * ratio) <= 1
                  ? "sharp"
                  : `canvas ${canvas.width}x${canvas.height}, css ${css.width}x${css.height} at ${ratio}`;
              }),
            { message: `the preview canvas at ${scale}%` },
          )
          .toBe("sharp");
        // And the timeline's.
        await expect
          .poll(
            () =>
              page.evaluate(() => {
                const canvas = document.querySelector<HTMLCanvasElement>(
                  '[role="region"][aria-label="Timeline"] canvas',
                );
                if (canvas === null) return "no timeline canvas";
                const css = canvas.getBoundingClientRect();
                const ratio = window.devicePixelRatio;
                return Math.abs(canvas.width - css.width * ratio) <= 1 &&
                  Math.abs(canvas.height - css.height * ratio) <= 1
                  ? "sharp"
                  : `canvas ${canvas.width}x${canvas.height}, css ${css.width}x${css.height} at ${ratio}`;
              }),
            { message: `the timeline canvas at ${scale}%` },
          )
          .toBe("sharp");

        // The clip, where the page draws it (CSS pixels), dragged 100 CSS px to the right.
        const g = await timelineGeometry(page);
        const clip = clipBox(g, clipAt(g, "V1", 2).id);
        const pointer = await hand(page);
        expect(pointer.k).toBeCloseTo(scale / 100, 2);
        // On a 20 px grid, so the pointer is on a whole point at 90% and at 125% too and the
        // three drags are the same drag.
        const grid = (value: number) => Math.round(value / 20) * 20;
        const from = { x: grid(clip.x + 40), y: grid(clip.y) };
        await pointer.drag(from, { x: from.x + 100, y: from.y });
        await expect.poll(async () => (await layout(page))[0]?.clips[0]?.[0]).not.toBe(2);
        await editorCaughtUp(page);
        const start = (await layout(page))[0]?.clips[0]?.[0] ?? Number.NaN;
        await snapshot(page, info, `dragged at ${scale}`);
        // Ctrl+Z puts it back for the next scale.
        await shortcut(app, "Control+Z");
        await expect.poll(() => layout(page)).toEqual(at(2));
        await editorCaughtUp(page);
        return (start - 2) * g.pxPerSec;
      };

      // 100 CSS px at the timeline's zoom, to the frame (30 fps), at every scale.
      const px100 = await moved(100);
      const px90 = await moved(90);
      const px125 = await moved(125);
      const zoom = (await timelineGeometry(page)).pxPerSec;
      const frame = zoom / 30;
      expect(Math.abs(px100 - 100)).toBeLessThanOrEqual(frame);
      expect(px90).toBeCloseTo(px100, 6);
      expect(px125).toBeCloseTo(px100, 6);
    } finally {
      await close();
      work.remove();
    }
  });
});
