// SPDX-License-Identifier: GPL-3.0-or-later
// F68 Keyframe lanes and the curve editor (docs/user-flows.md, WP-2C.05): three opacity keys
// from the Inspector's diamond; the toolbar's Keyframe lanes button opens the lane under the
// clip; a key dragged along the lane moves (one undo step); its menu sets Ease in-out; two
// undos restore the document; Bezier… opens the curve editor, where the arrow keys nudge a
// handle and a preset is picked; the nudge keys, Delete, a double-click on the lane, and Copy
// and Paste work on keys; Alt+Right steps to a key, Shift+F10 opens its menu and Alt+K closes
// the lanes. The menu and the dialog pass axe. Real clicks, drags and keys; only the OS file
// picker is stubbed.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import {
  clickClip,
  clipAt,
  createProject,
  drag,
  dropCard,
  editorCaughtUp,
  freeMainTrack,
  hasRealEngine,
  importWithButton,
  inspector,
  laneY,
  launchFlowApp,
  layout,
  movePlayhead,
  sec,
  sequence,
  sequenceFormat,
  shortcut,
  showTimeline,
  snapshot,
  type TimelineGeometry,
  timelineCanvas,
  timelineGeometry,
  timelineTools,
  toolbar,
  workspace,
  xAt,
} from "./helpers.ts";

const desktopDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");

interface Key {
  readonly t: number;
  readonly value: number;
  readonly interp: { kind: string; preset?: string | null; x1?: number; y1?: number };
}

/** The clip's opacity keys as the engine has them (none while it is static). */
async function opacityKeys(page: Page, id: string): Promise<Key[]> {
  const got = await editorCall<{ visual: { opacity: number | { keyframes: Key[] } } }>(
    page,
    "item.get",
    { item: id },
  );
  const opacity = got.visual.opacity;
  return typeof opacity === "number" ? [] : opacity.keyframes;
}

/** Key times in seconds on the timeline (the clip starts at 1 s). */
const times = async (page: Page, id: string): Promise<number[]> =>
  (await opacityKeys(page, id)).map((k) => Math.round((k.t / sec(1) + 1) * 1000) / 1000);

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

/** Page y of the first keyframe lane row under the clips of `track` (tokens read from the page). */
async function laneRowY(page: Page, g: TimelineGeometry, track: string): Promise<number> {
  const tokens = await page.evaluate(() => {
    const cs = getComputedStyle(document.documentElement);
    const px = (name: string) => Number.parseFloat(cs.getPropertyValue(name));
    return { trackMd: px("--track-md"), controlSm: px("--control-sm") };
  });
  return laneY(g, track) + tokens.trackMd / 2 + tokens.controlSm / 2;
}

test.describe("F68 keyframe lanes and the curve editor", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("add three keys, open the lane, drag a key, ease it, undo twice; curve editor, keys, copy and paste", async () => {
    test.setTimeout(300_000);
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await freeMainTrack(app, page); // clips stay where they are dropped
      await createProject(app, page, "Keyframes", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
      await editorCaughtUp(page);
      await showTimeline(page, 12);
      const id = clipAt(await timelineGeometry(page), "V1", 1).id;
      const undo = toolbar(page).getByRole("button", { name: /^Undo/ });
      const canvas = timelineCanvas(page).locator("canvas");
      const lanes = timelineTools(page).getByRole("button", { name: /^Keyframe lanes/ });

      // 1. Three opacity keys from the Inspector: the diamond at 2 s, a typed value at 4 s
      // (the property is animated now, so it keys), the diamond again at 6 s.
      await clickClip(page, id);
      await expect(lanes).toBeDisabled(); // nothing is animated yet
      const f = inspector(page);
      const keys = f.panel.getByRole("group", { name: "Keyframes for Opacity" });
      await movePlayhead(page, 2);
      await keys.getByRole("button", { name: "Add keyframe for Opacity" }).click();
      await expect.poll(() => times(page, id)).toEqual([2]);
      await movePlayhead(page, 4);
      await f.opacity.focus();
      await page.keyboard.press("Home");
      await expect.poll(() => times(page, id)).toEqual([2, 4]);
      await movePlayhead(page, 6);
      await keys.getByRole("button", { name: "Add keyframe at playhead for Opacity" }).click();
      await expect.poll(() => times(page, id)).toEqual([2, 4, 6]);
      expect((await opacityKeys(page, id)).map((k) => k.interp.kind)).toEqual([
        "linear",
        "linear",
        "linear",
      ]);
      const original = await opacityKeys(page, id);
      const originalSequence = JSON.stringify(await sequence(page));

      // 2. The toolbar's Keyframe lanes button opens the lane under the clip. View state: the
      // document does not change.
      await expect(lanes).toBeEnabled();
      await expect(lanes).toHaveAttribute("aria-pressed", "false");
      await lanes.click();
      await expect(lanes).toHaveAttribute("aria-pressed", "true");
      expect(await opacityKeys(page, id)).toEqual(original);
      let g = await timelineGeometry(page);
      const y = await laneRowY(page, g, "V1");
      await snapshot(page, info, "lane open");

      // 3. Drag the middle key from 4 s to 5 s along the lane: one undo step.
      await drag(page, { x: xAt(g, sec(4)), y }, { x: xAt(g, sec(5)), y });
      await expect.poll(() => times(page, id)).toEqual([2, 5, 6]);
      await expect(undo).toHaveAccessibleName("Undo Move keyframe");

      // 4. Its menu: Interpolation ▸ Ease in-out.
      const menu = page.getByRole("menu", { name: "Keyframe actions" });
      const easings = page.getByRole("menu", { name: "Interpolation" });
      const openEasings = async (atSec: number) => {
        g = await timelineGeometry(page);
        await page.mouse.click(xAt(g, sec(atSec)), y, { button: "right" });
        await expect(menu).toBeVisible();
        await menu.getByRole("menuitem", { name: /^Interpolation/ }).click();
        await expect(easings).toBeVisible();
      };
      await page.mouse.click(xAt(g, sec(5)), y, { button: "right" });
      await expect(menu).toBeVisible();
      expect(await violations(page, '[role="menu"]')).toEqual([]);
      await expect(menu.getByRole("menuitem", { name: "Copy" })).toBeVisible();
      await expect(menu.getByRole("menuitem", { name: "Delete keyframe" })).toBeVisible();
      await expect(menu.getByRole("menuitem", { name: "Go to keyframe" })).toBeVisible();
      await menu.getByRole("menuitem", { name: /^Interpolation/ }).click();
      await expect(easings).toBeVisible();
      await expect(easings.getByRole("menuitemradio", { name: "Linear" })).toBeChecked();
      await easings.getByRole("menuitemradio", { name: "Ease in-out" }).click();
      await expect
        .poll(async () => (await opacityKeys(page, id))[1]?.interp)
        .toMatchObject({ kind: "bezier", preset: "ease_in_out", x1: 0.42 });
      await expect(undo).toHaveAccessibleName("Undo Change easing");
      await snapshot(page, info, "key moved and eased");

      // 5. Undo twice: the document is as it was (the engine's item and its sequence).
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await opacityKeys(page, id))[1]?.interp.kind).toBe("linear");
      await shortcut(app, "Control+Z");
      await expect.poll(() => times(page, id)).toEqual([2, 4, 6]);
      expect(await opacityKeys(page, id)).toEqual(original);
      expect(JSON.stringify(await sequence(page))).toBe(originalSequence);
      // Redo twice: the key is at 5 s and eased again.
      await shortcut(app, "Control+Y");
      await shortcut(app, "Control+Y");
      await expect
        .poll(async () => (await opacityKeys(page, id))[1]?.interp.preset)
        .toBe("ease_in_out");
      await expect.poll(() => times(page, id)).toEqual([2, 5, 6]);

      // 6. Bezier… opens the curve editor: no taller than 60% of the window, it passes axe, the
      // arrow keys nudge a handle (one undo step) and a preset is picked.
      await openEasings(5);
      await expect(easings.getByRole("menuitemradio", { name: "Ease in-out" })).toBeChecked();
      await easings.getByRole("menuitem", { name: "Bezier…" }).click();
      const dialog = page.getByRole("dialog", { name: "Easing" });
      await expect(dialog).toBeVisible();
      const viewport = await page.evaluate(() => window.innerHeight);
      const size = await dialog.boundingBox();
      expect(size?.height ?? Number.POSITIVE_INFINITY).toBeLessThanOrEqual(0.6 * viewport + 1);
      expect(await violations(page, '[role="dialog"]')).toEqual([]);
      const preset = dialog.getByLabel("Preset");
      await expect(preset).toHaveValue("ease_in_out");
      const handle = dialog.getByRole("slider", { name: "Start handle" });
      await handle.focus();
      await page.keyboard.press("ArrowRight");
      await expect
        .poll(async () => (await opacityKeys(page, id))[1]?.interp)
        .toMatchObject({ kind: "bezier", x1: 0.43, y1: 0 });
      await expect(handle).toHaveAttribute("aria-valuetext", "time 0.43, value 0.00");
      await expect(preset).toHaveValue("");
      await snapshot(page, info, "curve editor");
      await preset.selectOption("back_out");
      await expect
        .poll(async () => (await opacityKeys(page, id))[1]?.interp.preset)
        .toBe("back_out");
      await dialog.getByRole("button", { name: "Done" }).click();
      await expect(dialog).toBeHidden();

      // 7. Keys on a selected key: `.` nudges it one frame, Delete removes it (the clip stays).
      const rate = (await sequenceFormat(page)).frame_rate;
      const frame = rate.den / rate.num;
      g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(6)), y);
      await canvas.focus();
      await page.keyboard.press(".");
      await expect.poll(async () => (await times(page, id))[2]).toBeCloseTo(6 + frame, 2);
      await expect(undo).toHaveAccessibleName("Undo Move keyframe");
      await page.keyboard.press("Delete");
      await expect.poll(() => times(page, id)).toEqual([2, 5]);
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await times(page, id)).length).toBe(3);
      await shortcut(app, "Control+Z");
      await expect.poll(() => times(page, id)).toEqual([2, 5, 6]);

      // 8. A double-click on the lane adds a key there.
      g = await timelineGeometry(page);
      await page.mouse.dblclick(xAt(g, sec(8)), y);
      await expect.poll(() => times(page, id)).toEqual([2, 5, 6, 8]);

      // 9. Copy the key at 2 s and paste it at the playhead (9 s): the same value there.
      await page.mouse.click(xAt(g, sec(2)), y);
      await shortcut(app, "Control+C");
      await movePlayhead(page, 9);
      await clickClip(page, id);
      await shortcut(app, "Control+V");
      await expect.poll(() => times(page, id)).toEqual([2, 5, 6, 8, 9]);
      const pasted = await opacityKeys(page, id);
      expect(pasted[4]?.value).toBe(pasted[0]?.value);
      // Ctrl+X takes the pasted key (it is selected), never its clip; Ctrl+V puts it back.
      await shortcut(app, "Control+X");
      await expect.poll(() => times(page, id)).toEqual([2, 5, 6, 8]);
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
      await shortcut(app, "Control+V");
      await expect.poll(() => times(page, id)).toEqual([2, 5, 6, 8, 9]);
      await snapshot(page, info, "keys added and pasted");

      // 10. The keyboard reaches the keys too: Alt+Right steps to the next key and the playhead
      // goes with it; Alt+K closes the lanes.
      await movePlayhead(page, 1);
      await clickClip(page, id);
      await canvas.focus();
      await page.keyboard.press("Alt+ArrowRight");
      await expect
        .poll(async () => (await editorCall<{ playhead: number }>(page, "view.get")).playhead)
        .toBe(sec(2));
      await page.keyboard.press("Alt+ArrowRight");
      await expect
        .poll(async () => (await editorCall<{ playhead: number }>(page, "view.get")).playhead)
        .toBe(sec(5));
      // Shift+F10 opens the selected key's menu, as a right-click on it does.
      await page.keyboard.press("Shift+F10");
      await expect(menu).toBeVisible();
      await expect(menu.getByRole("menuitem", { name: "Delete keyframe" })).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(menu).toBeHidden();
      await canvas.focus();
      await page.keyboard.press("Alt+k");
      await expect(lanes).toHaveAttribute("aria-pressed", "false");
      await page.keyboard.press("Alt+k");
      await expect(lanes).toHaveAttribute("aria-pressed", "true");
    } finally {
      await close();
      work.remove();
    }
  });
});
