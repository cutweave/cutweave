// SPDX-License-Identifier: GPL-3.0-or-later
// F69 Sequences: tabs, settings and the aspect switcher (docs/user-flows.md; WP-2B.03). A person
// with a landscape cut adds a vertical sequence from the "+" above the timeline, switches its
// shape in the preview bar, opens Sequence settings to type a size and choose a background
// (transparent, then a picture from the bin), renames the tab in place, duplicates it, closes
// and reopens a tab, moves between the tabs with the keys and the menu bar, deletes a sequence
// and takes that back, then saves, quits and finds the tabs as they were.
//
// Real clicks, keys and the app's own menus and dialog; only the OS file picker is stubbed. The
// document is read back from the engine (`project.info`, `timeline.get`, `view.get`).
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import type { Locator, Page } from "@playwright/test";
import { desktopDir, editorCall, expect, hasRealEngine, test } from "../fixtures.ts";
import {
  chooseMenu,
  createProject,
  dirtyDot,
  dropCard,
  editorCaughtUp,
  importWithButton,
  launchFlowApp,
  mainTrackIsMagnetic,
  openRecent,
  profileDir,
  removeDir,
  shortcut,
  snapshot,
  workspace,
} from "./helpers.ts";

const BOUND = { timeout: 20_000 };

interface Brief {
  id: string;
  name: string;
  item_count: number;
  format: {
    width: number;
    height: number;
    background_fill?: { kind: string; media?: string; fit?: string } | null;
  };
}

/** The project's sequences as the engine has them, by ID. */
const sequences = async (page: Page): Promise<Brief[]> =>
  (await editorCall<{ sequences: Brief[] }>(page, "project.info")).sequences;

const named = async (page: Page, name: string): Promise<Brief | undefined> =>
  (await sequences(page)).find((s) => s.name === name);

/** The sequence the engine shows (`view.get`): what the preview and playback are of. */
const shownId = async (page: Page): Promise<string | null> =>
  (await editorCall<{ active_sequence: string | null }>(page, "view.get")).active_sequence;

const strip = (page: Page): Locator => page.getByRole("tablist", { name: "Sequences" });
const tab = (page: Page, name: string): Locator =>
  strip(page).getByRole("tab", { name, exact: true });
const tabNames = (page: Page): Promise<string[]> => strip(page).getByRole("tab").allTextContents();
const selectedTab = (page: Page): Locator =>
  strip(page).locator('[role="tab"][aria-selected="true"]');

/** Width over height of the preview's frame, as drawn. */
async function frameRatio(page: Page): Promise<number> {
  const box = await page.locator(".cw-preview__frame").boundingBox();
  return box === null || box.height === 0 ? 0 : box.width / box.height;
}

/** The tab's menu (right-click), open. */
async function tabMenu(page: Page, name: string): Promise<Locator> {
  await tab(page, name).click({ button: "right" });
  const menu = page.getByRole("menu", { name: `Sequence actions for ${name}` });
  await expect(menu).toBeVisible();
  return menu;
}

/** The "+" menu above the timeline, open. */
async function addMenu(page: Page): Promise<Locator> {
  await page.getByRole("button", { name: "Add or open a sequence" }).click();
  const menu = page.getByRole("menu", { name: "Add or open a sequence" });
  await expect(menu).toBeVisible();
  return menu;
}

/** axe's findings inside the element `selector` names, as `rule: first node`. */
async function violations(page: Page, selector: string): Promise<string[]> {
  const source = readFileSync(
    createRequire(path.join(desktopDir, "..", "..", "packages", "editor", "package.json")).resolve(
      "axe-core/axe.min.js",
    ),
    "utf8",
  );
  await page.evaluate(source);
  return page.evaluate(async (sel) => {
    interface AxeResult {
      violations: { id: string; nodes: { html: string; failureSummary?: string }[] }[];
    }
    const axe = (globalThis as unknown as { axe: { run(context: Element): Promise<AxeResult> } })
      .axe;
    const root = document.querySelector(sel);
    if (root === null) return [`nothing matches ${sel}`];
    // It fades in: its colours are the ones the person reads once it has.
    await Promise.all(root.getAnimations({ subtree: true }).map((a) => a.finished));
    const results = await axe.run(root);
    return results.violations.map(
      (v) => `${v.id}: ${v.nodes[0]?.html ?? ""} ${v.nodes[0]?.failureSummary ?? ""}`,
    );
  }, selector);
}

test.describe("F69 sequence tabs, settings and aspect", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("add a vertical sequence, change its shape and background, manage the tabs, reopen", async () => {
    test.setTimeout(300_000);
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    try {
      const first = await launchFlowApp(work, home);
      let vertical = "";
      let main = "";
      try {
        const { app, page } = first;
        await createProject(app, page, "Promo", work.dir);
        await importWithButton(app, page, [work.file("interview.mp4"), work.file("logo.png")]);
        await dropCard(page, "interview.mp4", "V1", 0);
        await editorCaughtUp(page);

        // 1. One sequence, one tab, selected; the preview has its 16:9 shape.
        const only = await sequences(page);
        expect(only).toHaveLength(1);
        main = only[0]?.name ?? "";
        expect(main).not.toBe("");
        await expect(strip(page).getByRole("tab")).toHaveText([main]);
        await expect(tab(page, main)).toHaveAttribute("aria-selected", "true");
        await expect.poll(() => frameRatio(page)).toBeCloseTo(16 / 9, 1);
        expect(await violations(page, ".cw-seqtabs")).toEqual([]);

        // 2. "+" > 9:16: a second sequence with its own tab, shown at once; the timeline is
        // empty and the preview is upright. Its V1 is magnetic, as Settings > Timeline says.
        const add = await addMenu(page);
        await add.getByRole("menuitem", { name: "9:16 (1080 × 1920)" }).click();
        await expect(strip(page).getByRole("tab")).toHaveText([main, "Sequence 2"]);
        await expect(tab(page, "Sequence 2")).toHaveAttribute("aria-selected", "true");
        const made = await named(page, "Sequence 2");
        expect(made?.format).toMatchObject({ width: 1080, height: 1920 });
        expect(made?.item_count).toBe(0);
        vertical = made?.id ?? "";
        await expect.poll(() => shownId(page), BOUND).toBe(vertical);
        await expect.poll(() => frameRatio(page)).toBeCloseTo(9 / 16, 1);
        expect(await mainTrackIsMagnetic(page)).toBe(true);
        await snapshot(page, info, "vertical sequence");

        // 3. The aspect switcher: 1:1 gives the sequence the square preset and the preview
        // follows; one Ctrl+Z puts 9:16 back.
        const aspects = page.getByRole("radiogroup", { name: "Preview aspect ratio" });
        await expect(aspects.getByRole("radio", { name: "9:16" })).toHaveAttribute(
          "aria-checked",
          "true",
        );
        await aspects.getByRole("radio", { name: "1:1" }).click();
        await expect.poll(async () => (await named(page, "Sequence 2"))?.format.width).toBe(1080);
        await expect.poll(async () => (await named(page, "Sequence 2"))?.format.height).toBe(1080);
        await expect.poll(() => frameRatio(page)).toBeCloseTo(1, 1);
        await shortcut(app, "Control+Z");
        await expect.poll(async () => (await named(page, "Sequence 2"))?.format.height).toBe(1920);
        await expect.poll(() => frameRatio(page)).toBeCloseTo(9 / 16, 1);

        // 4. The size menu lists every preset; Custom size… opens Sequence settings: no taller
        // than 60% of the window, every control named (axe).
        await page.getByRole("button", { name: "More sizes and sequence settings" }).click();
        const sizes = page.getByRole("menu", { name: "More sizes and sequence settings" });
        await expect(
          sizes.getByRole("menuitemradio", { name: "9:16 (1080 × 1920)" }),
        ).toBeChecked();
        await expect(sizes.getByRole("menuitemradio")).toHaveCount(6);
        await sizes.getByRole("menuitem", { name: "Custom size…" }).click();
        const dialog = page.getByRole("dialog", { name: "Sequence settings" });
        await expect(dialog).toBeVisible();
        const viewport = await page.evaluate(() => window.innerHeight);
        const size = await dialog.boundingBox();
        expect(size?.height ?? Number.POSITIVE_INFINITY).toBeLessThanOrEqual(0.6 * viewport + 1);
        expect(await violations(page, '[role="dialog"]')).toEqual([]);

        // A typed width; a transparent background; then the picture from the bin, shown whole.
        await dialog.getByRole("spinbutton", { name: "Width" }).dblclick();
        await page.keyboard.type("1000");
        await page.keyboard.press("Enter");
        await expect.poll(async () => (await named(page, "Sequence 2"))?.format.width).toBe(1000);
        const fill = dialog.getByLabel("Fill", { exact: true });
        await fill.selectOption({ label: "Transparent" });
        await expect
          .poll(async () => (await named(page, "Sequence 2"))?.format.background_fill?.kind)
          .toBe("transparent");
        await fill.selectOption({ label: "Image" });
        await expect
          .poll(async () => (await named(page, "Sequence 2"))?.format.background_fill?.kind)
          .toBe("image");
        await expect(
          dialog.getByLabel("Image", { exact: true }).locator("option:checked"),
        ).toHaveText("logo.png");
        await dialog.getByLabel("Fit", { exact: true }).selectOption({ label: "Show all of it" });
        await expect
          .poll(async () => (await named(page, "Sequence 2"))?.format.background_fill?.fit)
          .toBe("contain");
        expect(await violations(page, '[role="dialog"]')).toEqual([]);
        await snapshot(page, info, "sequence settings");
        // Each was one undo step: one Ctrl+Z takes the fit back, the image stays.
        await shortcut(app, "Control+Z");
        await expect
          .poll(
            async () => (await named(page, "Sequence 2"))?.format.background_fill?.fit ?? "cover",
          )
          .toBe("cover");
        await dialog.getByRole("button", { name: "Done" }).click();
        await expect(dialog).toBeHidden();

        // 5. A double-click renames the tab in place.
        await tab(page, "Sequence 2").dblclick();
        const field = page.getByRole("textbox", { name: "Sequence name" });
        await expect(field).toBeFocused();
        await field.fill("Reel");
        await page.keyboard.press("Enter");
        await expect(strip(page).getByRole("tab")).toHaveText([main, "Reel"]);
        expect((await named(page, "Reel"))?.id).toBe(vertical);

        // 6. The tab's menu: Duplicate puts the copy next to it and shows it; Aspect lists the
        // presets; Close hides the copy's tab and deletes nothing.
        await (await tabMenu(page, "Reel")).getByRole("menuitem", { name: "Duplicate" }).click();
        await expect(strip(page).getByRole("tab")).toHaveText([main, "Reel", "Reel copy"]);
        await expect(tab(page, "Reel copy")).toHaveAttribute("aria-selected", "true");
        const copy = (await named(page, "Reel copy"))?.id ?? "";
        expect(copy).not.toBe("");
        await expect.poll(() => shownId(page), BOUND).toBe(copy);
        const menu = await tabMenu(page, "Reel copy");
        expect(await violations(page, '[role="menu"]')).toEqual([]);
        await menu.getByRole("menuitem", { name: "Aspect" }).hover();
        await expect(page.getByRole("menuitemradio", { name: "16:9 (1920 × 1080)" })).toBeVisible();
        await page.keyboard.press("Escape");
        await page.keyboard.press("Escape");
        await expect(menu).toBeHidden();
        await (await tabMenu(page, "Reel copy"))
          .getByRole("menuitem", { name: "Close tab" })
          .click();
        await expect(strip(page).getByRole("tab")).toHaveText([main, "Reel"]);
        expect((await sequences(page)).map((s) => s.name).sort()).toEqual(
          [main, "Reel", "Reel copy"].sort(),
        );
        // Its neighbour shows now.
        await expect(tab(page, "Reel")).toHaveAttribute("aria-selected", "true");

        // "+" lists the closed sequence and brings its tab back.
        const reopen = await addMenu(page);
        await reopen.getByRole("menuitem", { name: "Reel copy" }).click();
        await expect(strip(page).getByRole("tab")).toHaveText([main, "Reel", "Reel copy"]);
        await expect(tab(page, "Reel copy")).toHaveAttribute("aria-selected", "true");

        // 7. A click shows the landscape cut again: its clip, its shape. Ctrl+PageDown and
        // Ctrl+PageUp walk the tabs; View > Next sequence does the same.
        await tab(page, main).click();
        await expect.poll(() => frameRatio(page)).toBeCloseTo(16 / 9, 1);
        await expect.poll(() => shownId(page), BOUND).toBe(only[0]?.id);
        await shortcut(app, "Control+PageDown");
        await expect(tab(page, "Reel")).toHaveAttribute("aria-selected", "true");
        await expect.poll(() => frameRatio(page)).toBeLessThan(1);
        await shortcut(app, "Control+PageUp");
        await expect(tab(page, main)).toHaveAttribute("aria-selected", "true");
        await chooseMenu(page, app, "View", /^Next sequence/);
        await expect(tab(page, "Reel")).toHaveAttribute("aria-selected", "true");
        // The arrow keys move along the strip while a tab has the focus.
        await tab(page, "Reel").focus();
        await page.keyboard.press("ArrowRight");
        await expect(tab(page, "Reel copy")).toHaveAttribute("aria-selected", "true");
        await expect(tab(page, "Reel copy")).toBeFocused();

        // 8. Delete sequence removes the copy with its tab; Ctrl+Z brings it back.
        await (await tabMenu(page, "Reel copy"))
          .getByRole("menuitem", { name: "Delete sequence" })
          .click();
        await expect(strip(page).getByRole("tab")).toHaveText([main, "Reel"]);
        await expect.poll(async () => (await sequences(page)).length).toBe(2);
        await shortcut(app, "Control+Z");
        await expect.poll(async () => (await sequences(page)).length).toBe(3);
        await expect(strip(page).getByRole("tab")).toHaveText([main, "Reel", "Reel copy"]);

        // 9. File > Sequence settings… opens the dialog for the sequence that shows.
        await tab(page, "Reel").click();
        await chooseMenu(page, app, "File", "Sequence settings…");
        await expect(dialog).toBeVisible();
        await expect(dialog.getByRole("spinbutton", { name: "Width" })).toHaveValue(/1000/);
        await page.keyboard.press("Escape");
        await expect(dialog).toBeHidden();

        // 10. The tabs as the person leaves them: the copy first, the landscape cut closed, Reel
        // showing. Save and quit.
        await (await tabMenu(page, "Reel copy"))
          .getByRole("menuitem", { name: "Move left" })
          .click();
        await expect(strip(page).getByRole("tab")).toHaveText([main, "Reel copy", "Reel"]);
        await (await tabMenu(page, main)).getByRole("menuitem", { name: "Close tab" }).click();
        await expect(strip(page).getByRole("tab")).toHaveText(["Reel copy", "Reel"]);
        await tab(page, "Reel").click();
        await expect(selectedTab(page)).toHaveText("Reel");
        await expect.poll(() => shownId(page), BOUND).toBe(vertical);
        // The engine has the tabs (they ride the panel layout, sent half a second after a change).
        await expect
          .poll(async () => {
            const view = await editorCall<{
              panel_layout: { sequenceTabs?: { closed: string[] } } | null;
            }>(page, "view.get");
            return view.panel_layout?.sequenceTabs?.closed.length ?? 0;
          }, BOUND)
          .toBe(1);
        await snapshot(page, info, "tabs before quitting");
        await shortcut(app, "Control+S");
        await expect(dirtyDot(page)).toBeHidden();
        const closed = app.waitForEvent("close");
        await chooseMenu(page, app, "File", "Exit");
        await closed;
      } finally {
        await first.close();
      }

      // 11. Another launch, the project reopened: the same tabs in the same order, the same one
      // showing, in the timeline, the preview and the engine.
      const second = await launchFlowApp(work, home);
      try {
        const { page } = second;
        await openRecent(page, "Promo.cutweave");
        await expect(page).toHaveTitle("Promo — Cutweave");
        await expect(strip(page).getByRole("tab")).toHaveText(["Reel copy", "Reel"], BOUND);
        await expect(selectedTab(page)).toHaveText("Reel");
        expect(await tabNames(page)).toEqual(["Reel copy", "Reel"]);
        await expect.poll(() => shownId(page), BOUND).toBe(vertical);
        await expect.poll(() => frameRatio(page)).toBeLessThan(1);
        // The closed one is still in the project, and "+" reopens it.
        expect((await sequences(page)).map((s) => s.name)).toContain(main);
        const add = await addMenu(page);
        await expect(add.getByRole("menuitem", { name: main, exact: true })).toBeVisible();
        await page.keyboard.press("Escape");
        await snapshot(page, info, "reopened");
      } finally {
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
