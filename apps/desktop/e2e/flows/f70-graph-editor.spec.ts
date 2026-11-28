// SPDX-License-Identifier: GPL-3.0-or-later
// F70 Graph editor (docs/user-flows.md, WP-2C.07): a position animation of three keys made in
// the Inspector; View > Panels > Graph editor shows its curves; a key is clicked (its time and
// value show), made a bezier, and its handle dragged: the stored numbers are where the pointer
// went (within 1e-3), one undo restores them; mirrored tangents move the handle across the key
// in the same undo step; a key is dragged in time and value, kept to one axis with Shift,
// snapped to the playhead and not snapped with snapping off; a box selects two keys and the
// arrow keys nudge them; the Time and Value fields take typed numbers; a double-click adds a
// key and Delete removes it; Alt+arrows, Shift+F10 and Enter work from the keyboard; the wheel
// zooms and F fits; the normalized view; 150% interface scale; the panel in its own window.
// The panel passes axe. Real clicks, drags and keys; only the OS file picker is stubbed.
//
// That the curve drawn is the engine's curve (`keyframe.eval`, within 1e-6) is asserted where
// the drawn samples can be read: packages/editor/src/panels/graph/graph.integration.test.ts.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import type { ElectronApplication, Locator, Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import {
  clickClip,
  clipAt,
  createProject,
  dropCard,
  editorCaughtUp,
  FRAME,
  freeMainTrack,
  hasRealEngine,
  importWithButton,
  inspector,
  launchFlowApp,
  layout,
  movePlayhead,
  sec,
  sequence,
  shortcut,
  showTimeline,
  snapshot,
  timelineGeometry,
  toolbar,
  workspace,
} from "./helpers.ts";

const desktopDir = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");

interface Bezier {
  kind: string;
  x1: number;
  y1: number;
  x2: number;
  y2: number;
  preset?: string | null;
}
interface Key {
  readonly t: number;
  readonly value: { x: number; y: number };
  readonly interp: Bezier;
}

/** The clip's position keys as the engine has them. */
async function positionKeys(page: Page, id: string): Promise<Key[]> {
  const got = await editorCall<{
    visual: { transform: { position: { keyframes?: Key[] } } };
  }>(page, "item.get", { item: id });
  return got.visual.transform.position.keyframes ?? [];
}

/** Key times in seconds on the timeline (the clip starts at 1 s), to the millisecond. */
const times = async (page: Page, id: string): Promise<number[]> =>
  (await positionKeys(page, id)).map((k) => Math.round((k.t / sec(1) + 1) * 1000) / 1000);
const xs = async (page: Page, id: string): Promise<number[]> =>
  (await positionKeys(page, id)).map((k) => k.value.x);

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
    const results = await axe.run(root);
    return results.violations.map(
      (v) => `${v.id}: ${v.nodes[0]?.html ?? ""} ${v.nodes[0]?.failureSummary ?? ""}`,
    );
  }, selector);
}

// ---- the graph's canvas ----------------------------------------------------------------------

const graphCanvas = (page: Page): Locator =>
  page.getByRole("application", { name: "Graph editor" });

/** Where the graph draws: its canvas on the page (CSS pixels), and the time and value it shows. */
interface Graph {
  readonly x: number;
  readonly y: number;
  readonly t0: number;
  readonly t1: number;
  readonly v0: number;
  readonly v1: number;
  readonly left: number;
  readonly top: number;
  readonly width: number;
  readonly height: number;
}

/**
 * The canvas says what it shows (`data-viewport`: the time and value window and the plot's box),
 * which is what a person reads off its two rulers.
 */
async function graph(page: Page): Promise<Graph> {
  const canvas = graphCanvas(page);
  await expect(canvas).toHaveAttribute("data-viewport", /\S/);
  return canvas.evaluate((el) => {
    const r = el.getBoundingClientRect();
    const [t0, t1, v0, v1, left, top, width, height] = (el.getAttribute("data-viewport") ?? "")
      .split(" ")
      .map(Number) as [number, number, number, number, number, number, number, number];
    return { x: r.x, y: r.y, t0, t1, v0, v1, left, top, width, height };
  });
}

/** Resolves with the graph once what it shows has stopped changing (a zoom, a fit, a reopen). */
async function settledGraph(page: Page): Promise<Graph> {
  let last = "";
  await expect
    .poll(async () => {
      const now = JSON.stringify(await graph(page));
      const same = now === last;
      last = now;
      return same;
    })
    .toBe(true);
  return graph(page);
}

/** The whole page pixel at sequence time `seconds` and value `v`. */
const at = (g: Graph, seconds: number, v: number): { x: number; y: number } => ({
  x: Math.round(g.x + g.left + ((sec(seconds) - g.t0) / (g.t1 - g.t0)) * g.width),
  y: Math.round(g.y + g.top + ((g.v1 - v) / (g.v1 - g.v0)) * g.height),
});
/** The sequence time (seconds) and the value under a page pixel. */
const secondsAt = (g: Graph, x: number): number =>
  (g.t0 + ((x - g.x - g.left) / g.width) * (g.t1 - g.t0)) / sec(1);
const valueAt = (g: Graph, y: number): number =>
  g.v1 - ((y - g.y - g.top) / g.height) * (g.v1 - g.v0);
/** Seconds of sequence time one page pixel is worth. */
const secondsPerPx = (g: Graph): number => (g.t1 - g.t0) / g.width / sec(1);
/** The nearest frame to a time, in seconds (30 fps). */
const onFrame = (seconds: number): number =>
  Math.round((Math.round((seconds * sec(1)) / FRAME) * FRAME * 1000) / sec(1)) / 1000;
/** What a drag of `dy` page pixels changes a value by: a tenth of a pixel's worth, rounded. */
function valueChange(g: Graph, dy: number): number {
  const perPx = (g.v1 - g.v0) / g.height;
  const q = 10 ** (Math.floor(Math.log10(perPx)) - 1);
  return Number((Math.round((-dy * perPx) / q) * q).toPrecision(12));
}

/** A pointer drag in small steps, like a hand, between two whole pixels. */
async function dragTo(
  page: Page,
  from: { x: number; y: number },
  to: { x: number; y: number },
): Promise<void> {
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move(from.x + Math.sign(to.x - from.x || 1) * 4, from.y, { steps: 2 });
  await page.mouse.move(to.x, to.y, { steps: 12 });
  await page.mouse.up();
}

const tools = (page: Page) => {
  const panel = page.locator(".cw-graph-panel");
  return {
    panel,
    time: panel.getByRole("textbox", { name: "Time" }),
    value: panel.getByRole("textbox", { name: "Value" }),
    button: (name: string) => panel.getByRole("button", { name, exact: true }),
    tangent: (name: string) => panel.getByRole("radio", { name, exact: true }),
  };
};

/** View > Panels > Graph editor, with real clicks (it toggles). */
async function toggleGraphPanel(page: Page): Promise<void> {
  await page
    .getByRole("menubar", { name: "Application menu" })
    .getByRole("menuitem", { name: "View", exact: true })
    .click();
  await page.getByRole("menu", { name: "View" }).getByRole("menuitem", { name: "Panels" }).click();
  const list = page.getByRole("menu", { name: "Panels" });
  await expect(list).toBeVisible();
  await list.getByRole("menuitemcheckbox", { name: "Graph editor", exact: true }).click();
  await expect(page.getByRole("menu")).toHaveCount(0);
}

/** Settings > Appearance > Interface scale, then Done: resolves once the window has the zoom. */
async function setScale(app: ElectronApplication, page: Page, scale: number): Promise<void> {
  await shortcut(app, "Control+,");
  const settings = page.getByRole("dialog", { name: "Settings" });
  await expect(settings).toBeVisible();
  await settings.getByRole("tab", { name: "Appearance" }).click();
  const radio = settings
    .getByRole("radiogroup", { name: "Interface scale" })
    .getByRole("radio", { name: `${scale}%`, exact: true });
  await radio.click();
  await expect(radio).toHaveAttribute("aria-checked", "true");
  await expect
    .poll(() =>
      app.evaluate(
        ({ BrowserWindow }) =>
          BrowserWindow.getAllWindows()
            .find((w) => !w.webContents.getURL().endsWith("/popout.html"))
            ?.webContents.getZoomFactor() ?? 0,
      ),
    )
    .toBeCloseTo(scale / 100, 3);
  await settings.getByRole("button", { name: "Done" }).click();
  await expect(settings).toBeHidden();
}

test.describe("F70 graph editor", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("curves of a position animation: handles, tangent modes, key drags, box select, numbers, keyboard, zoom, detached", async () => {
    test.setTimeout(480_000);
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await freeMainTrack(app, page); // clips stay where they are dropped
      await createProject(app, page, "Graph", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
      await editorCaughtUp(page);
      await showTimeline(page, 12);
      const id = clipAt(await timelineGeometry(page), "V1", 1).id;
      const undo = toolbar(page).getByRole("button", { name: /^Undo/ });

      // 1. A position animation of three keys from the Inspector: X is -300 at 2 s (the
      // diamond), 300 at 4 s and 100 at 6 s (typed: the property is animated, so they key).
      await clickClip(page, id);
      const f = inspector(page);
      const typeX = async (value: string) => {
        await f.positionX.dblclick();
        await page.keyboard.type(value);
        await page.keyboard.press("Enter");
      };
      await typeX("-300");
      await movePlayhead(page, 2);
      await f.panel
        .getByRole("group", { name: "Keyframes for Position" })
        .getByRole("button", { name: "Add keyframe for Position" })
        .click();
      await expect.poll(() => times(page, id)).toEqual([2]);
      await movePlayhead(page, 4);
      await typeX("300");
      await expect.poll(() => times(page, id)).toEqual([2, 4]);
      await movePlayhead(page, 6);
      await typeX("100");
      await expect.poll(() => times(page, id)).toEqual([2, 4, 6]);
      expect(await xs(page, id)).toEqual([-300, 300, 100]);
      const original = await positionKeys(page, id);
      expect(original.map((k) => k.interp.kind)).toEqual(["linear", "linear", "linear"]);
      const originalSequence = JSON.stringify(await sequence(page));

      // 2. View > Panels > Graph editor: a tab beside the Timeline with the clip's curves. View
      // state: the document does not change.
      await toggleGraphPanel(page);
      const t = tools(page);
      await expect(t.panel).toBeVisible();
      await expect(t.panel.getByRole("list", { name: "Curves" })).toContainText("Position");
      await expect(t.button("Hold")).toBeDisabled(); // nothing is selected yet
      expect(JSON.stringify(await sequence(page))).toBe(originalSequence);
      let g = await settledGraph(page);
      // Every key is inside what the graph shows.
      expect(g.t0).toBeLessThan(sec(2));
      expect(g.t1).toBeGreaterThan(sec(6));
      expect(g.v0).toBeLessThan(-300);
      expect(g.v1).toBeGreaterThan(300);
      await snapshot(page, info, "graph editor open");

      // 3. A click on the middle key of X selects it: its time and value show.
      const middle = at(g, 4, 300);
      await page.mouse.click(middle.x, middle.y);
      await expect(t.time).toHaveValue("00:00:04:00");
      await expect(t.value).toHaveValue("300");
      await expect(t.button("Linear")).toHaveAttribute("aria-pressed", "true");

      // 4. Bezier: the segment keeps its straight shape and gets handles. The first key too.
      await t.button("Bezier").click();
      await expect.poll(async () => (await positionKeys(page, id))[1]?.interp.kind).toBe("bezier");
      await expect(undo).toHaveAccessibleName("Undo Change easing");
      const first = at(g, 2, -300);
      await page.mouse.click(first.x, first.y);
      await expect(t.time).toHaveValue("00:00:02:00");
      await t.button("Bezier").click();
      await expect.poll(async () => (await positionKeys(page, id))[0]?.interp.kind).toBe("bezier");
      const straight = await positionKeys(page, id);
      expect(straight[1]?.interp).toMatchObject({ x1: 0.333333, y1: 0.333333 });

      // 5. The middle key's out handle dragged: the stored numbers are where the pointer went,
      // within 1e-3, the segment before the key is untouched (free tangents), one undo step.
      await page.mouse.click(middle.x, middle.y);
      await expect(t.time).toHaveValue("00:00:04:00");
      // The handle: a third of the way from the key (4 s, 300) to the next (6 s, 100).
      const handle = at(g, 4 + 2 * 0.333333, 300 - 200 * 0.333333);
      const target = at(g, 4.5, 250);
      await dragTo(page, handle, target);
      const wantX1 = (secondsAt(g, target.x) - 4) / 2;
      const wantY1 = (valueAt(g, target.y) - 300) / -200;
      await expect
        .poll(async () => Math.abs(((await positionKeys(page, id))[1]?.interp.x1 ?? 0) - wantX1))
        .toBeLessThan(1e-3);
      const dragged = await positionKeys(page, id);
      expect(Math.abs((dragged[1]?.interp.y1 ?? 0) - wantY1)).toBeLessThan(1e-3);
      expect(dragged[1]?.interp.x2).toBe(0.666667);
      expect(dragged[0]).toEqual(straight[0]);
      await expect(undo).toHaveAccessibleName("Undo Adjust curve handle");
      await snapshot(page, info, "handle dragged");
      await shortcut(app, "Control+Z");
      await expect.poll(() => positionKeys(page, id)).toEqual(straight);
      await shortcut(app, "Control+Y");
      await expect.poll(() => positionKeys(page, id)).toEqual(dragged);

      // 6. Mirrored: dragging the out handle turns the in handle onto the same line with the
      // same reach, and both are one undo step.
      await t.tangent("Mirrored").click();
      await expect(t.tangent("Mirrored")).toBeChecked();
      const target2 = at(g, 4.8, 280);
      await dragTo(page, target, target2);
      await expect
        .poll(async () => (await positionKeys(page, id))[0]?.interp.x2)
        .not.toBe(0.666667);
      const mirrored = await positionKeys(page, id);
      const out = mirrored[1]?.interp as Bezier;
      const into = mirrored[0]?.interp as Bezier;
      expect(Math.abs(out.x1 - (secondsAt(g, target2.x) - 4) / 2)).toBeLessThan(1e-3);
      // The same reach in time on both sides of the key (both segments are 2 s long)...
      expect(Math.abs(1 - into.x2 - out.x1)).toBeLessThan(1e-3);
      // ...and the same slope: value per second leaving the key and arriving at it.
      const slopeOut = (out.y1 * -200) / (out.x1 * 2);
      const slopeIn = ((1 - into.y2) * 600) / ((1 - into.x2) * 2);
      expect(Math.abs(slopeIn - slopeOut)).toBeLessThan(0.5);
      await shortcut(app, "Control+Z");
      await expect.poll(() => positionKeys(page, id)).toEqual(dragged);
      await t.tangent("Free").click();

      // 7. The last key dragged to 5.5 s and up: it lands on the frame, its value follows the
      // pointer, the easing of the keys stays; one undo step.
      const last = at(g, 6, 100);
      const lastTo = at(g, 5.5, 180);
      await dragTo(page, last, lastTo);
      await expect.poll(() => times(page, id)).toEqual([2, 4, 5.5]);
      const wantValue = 100 + valueChange(g, lastTo.y - last.y);
      expect((await xs(page, id))[2]).toBeCloseTo(wantValue, 6);
      expect((await positionKeys(page, id))[2]?.value.y).toBe(0); // Y's curve was not dragged
      await expect(undo).toHaveAccessibleName("Undo Move keyframe");
      await expect(t.time).toHaveValue("00:00:05:15");
      // With Shift the drag keeps to the axis it moved further along: time only.
      await page.keyboard.down("Shift");
      await dragTo(page, lastTo, { x: lastTo.x - 60, y: lastTo.y - 8 });
      await page.keyboard.up("Shift");
      await expect
        .poll(async () => (await times(page, id))[2])
        .toBe(onFrame(5.5 - 60 * secondsPerPx(g)));
      expect((await xs(page, id))[2]).toBeCloseTo(wantValue, 6);
      await shortcut(app, "Control+Z");
      await shortcut(app, "Control+Z");
      await expect.poll(() => positionKeys(page, id)).toEqual(dragged);

      // 8. Snapping: the playhead goes to 5 s from the graph's own time ruler; the last key
      // dragged to a few pixels from it lands exactly on it; with snapping off it lands on the
      // frame under the pointer.
      await page.mouse.click(at(g, 5, 0).x, g.y + g.top / 2);
      await expect
        .poll(async () => (await editorCall<{ playhead: number }>(page, "view.get")).playhead)
        .toBe(sec(5));
      const near = { x: at(g, 5, 100).x + 6, y: last.y };
      // Where the pointer is, frame by frame: not 5 s.
      const unsnapped = onFrame(6 + (near.x - last.x) * secondsPerPx(g));
      expect(unsnapped).not.toBe(5);
      await dragTo(page, last, near);
      await expect.poll(() => times(page, id)).toEqual([2, 4, 5]);
      await shortcut(app, "Control+Z");
      await expect.poll(() => times(page, id)).toEqual([2, 4, 6]);
      await t.button("Snapping").click();
      await expect(t.button("Snapping")).toHaveAttribute("aria-pressed", "false");
      await dragTo(page, last, near);
      await expect.poll(() => times(page, id)).toEqual([2, 4, unsnapped]);
      await shortcut(app, "Control+Z");
      await expect.poll(() => positionKeys(page, id)).toEqual(dragged);
      await t.button("Snapping").click();
      await expect(t.button("Snapping")).toHaveAttribute("aria-pressed", "true");

      // 9. A box around the last two keys of X selects both; the arrow keys nudge them: Right by
      // a frame, Up by one in value (X only: Y's points were not in the box).
      const corner = at(g, 3.5, 340);
      const opposite = at(g, 6.3, 40);
      await dragTo(page, corner, opposite);
      await expect(t.time).toBeDisabled(); // two keys: no one time
      await expect(t.value).toHaveAttribute("placeholder", "Mixed");
      await page.keyboard.press("ArrowRight");
      const frame = Math.round((FRAME * 1000) / sec(1)) / 1000;
      await expect.poll(() => times(page, id)).toEqual([2, onFrame(4 + frame), onFrame(6 + frame)]);
      await expect(undo).toHaveAccessibleName("Undo Move keyframe");
      await shortcut(app, "Control+Z");
      await expect.poll(() => times(page, id)).toEqual([2, 4, 6]);
      await page.keyboard.press("ArrowUp");
      await expect.poll(() => xs(page, id)).toEqual([-300, 301, 101]);
      expect((await positionKeys(page, id)).map((k) => k.value.y)).toEqual([0, 0, 0]);
      await shortcut(app, "Control+Z");
      await expect.poll(() => positionKeys(page, id)).toEqual(dragged);

      // 10. Typed numbers: the middle key's value, then its time.
      await page.mouse.click(middle.x, middle.y);
      await expect(t.value).toHaveValue("300");
      await t.value.fill("250");
      await t.value.press("Enter");
      await expect.poll(() => xs(page, id)).toEqual([-300, 250, 100]);
      await expect(t.value).toHaveValue("250");
      await t.time.fill("00:00:04:15");
      await t.time.press("Enter");
      await expect.poll(() => times(page, id)).toEqual([2, 4.5, 6]);
      await expect(t.time).toHaveValue("00:00:04:15");
      await t.time.fill("later");
      await t.time.press("Enter");
      await expect(t.time).toHaveAttribute("aria-invalid", "true");
      await t.time.press("Escape");
      await expect(t.time).toHaveValue("00:00:04:15");
      await shortcut(app, "Control+Z");
      await shortcut(app, "Control+Z");
      await expect.poll(() => positionKeys(page, id)).toEqual(dragged);

      // 11. A double-click on the curve at 3 s adds a key there (it is selected); Delete removes it.
      const on = await editorCall<{ samples: { value: { x: number } }[] }>(page, "keyframe.eval", {
        item: id,
        prop: "transform.position",
        from: sec(3),
        to: sec(3) + 1,
        samples: 2,
      });
      const curve = at(g, 3, on.samples[0]?.value.x ?? 0);
      await page.mouse.dblclick(curve.x, curve.y);
      await expect.poll(() => times(page, id)).toEqual([2, 3, 4, 6]);
      await expect(t.time).toHaveValue("00:00:03:00");
      await page.keyboard.press("Delete");
      await expect.poll(() => times(page, id)).toEqual([2, 4, 6]);

      // 12. From the keyboard: Alt+Right selects the first key at or after the playhead (5 s),
      // Alt+Left the one before it; Shift+F10 opens the key's menu; Enter opens the easing.
      await graphCanvas(page).focus();
      await page.keyboard.press("Alt+ArrowRight");
      await expect(t.time).toHaveValue("00:00:06:00");
      await page.keyboard.press("Alt+ArrowLeft");
      await expect(t.time).toHaveValue("00:00:04:00");
      await page.keyboard.press("Shift+F10");
      const menu = page.getByRole("menu", { name: "Keyframe actions" });
      await expect(menu).toBeVisible();
      await expect(menu.getByRole("menuitem", { name: "Delete keyframe" })).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(menu).toBeHidden();
      await graphCanvas(page).focus();
      await page.keyboard.press("Enter");
      const dialog = page.getByRole("dialog", { name: "Easing" });
      await expect(dialog).toBeVisible();
      const viewport = await page.evaluate(() => window.innerHeight);
      expect((await dialog.boundingBox())?.height ?? Infinity).toBeLessThanOrEqual(
        0.6 * viewport + 1,
      );
      await dialog.getByRole("button", { name: "Done" }).click();
      await expect(dialog).toBeHidden();
      // A right-click on a key opens the same menu, there.
      await page.mouse.click(first.x, first.y, { button: "right" });
      await expect(menu).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(menu).toBeHidden();

      // 13. Ctrl+wheel zooms time about the pointer, the wheel pans, F fits again.
      await page.mouse.move(middle.x, middle.y);
      await page.keyboard.down("Control");
      await page.mouse.wheel(0, -300);
      await page.keyboard.up("Control");
      const zoomed = await settledGraph(page);
      expect(zoomed.t1 - zoomed.t0).toBeLessThan(g.t1 - g.t0);
      expect(zoomed.v1 - zoomed.v0).toBeCloseTo(g.v1 - g.v0, 6);
      // The time under the pointer stayed under it.
      expect(secondsAt(zoomed, middle.x)).toBeCloseTo(secondsAt(g, middle.x), 3);
      await page.mouse.wheel(0, 120);
      const panned = await settledGraph(page);
      expect(panned.v0).not.toBeCloseTo(zoomed.v0, 3);
      await graphCanvas(page).focus();
      await page.keyboard.press("f");
      const fitted = await settledGraph(page);
      expect(fitted.t0).toBeCloseTo(g.t0, 0);
      expect(fitted.t1).toBeCloseTo(g.t1, 0);
      expect(fitted.v1).toBeCloseTo(g.v1, 6);
      g = fitted;
      expect(await violations(page, ".cw-graph-panel")).toEqual([]);

      // 14. Normalized: every curve runs from its lowest key to its highest, and a key is still
      // where it is drawn (X's highest key, 300, is at the top).
      await t.button("Normalize values").click();
      const normalized = await settledGraph(page);
      expect(normalized.v0).toBeCloseTo(-0.1, 3);
      expect(normalized.v1).toBeCloseTo(1.1, 3);
      const top = at(normalized, 4, 1);
      await page.mouse.click(first.x, at(normalized, 2, 0).y);
      await expect(t.value).toHaveValue("-300");
      await page.mouse.click(top.x, top.y);
      await expect(t.value).toHaveValue("300");
      await snapshot(page, info, "normalized");
      await t.button("Normalize values").click();
      g = await settledGraph(page);

      // 15. At 150% interface scale a click still lands on the key under it, and a drag moves
      // it by what the pointer moved.
      await setScale(app, page, 150);
      const big = await settledGraph(page);
      const bigLast = at(big, 6, 100);
      await page.mouse.click(bigLast.x, bigLast.y);
      await expect(t.time).toHaveValue("00:00:06:00");
      await expect(t.value).toHaveValue("100");
      const bigTo = at(big, 5, 100);
      await page.keyboard.down("Shift");
      await dragTo(page, bigLast, bigTo);
      await page.keyboard.up("Shift");
      await expect.poll(() => times(page, id)).toEqual([2, 4, 5]);
      await shortcut(app, "Control+Z");
      await expect.poll(() => positionKeys(page, id)).toEqual(dragged);
      await setScale(app, page, 100);

      // 16. In a window of its own: the same canvas, clicked and nudged there.
      const opened = app.waitForEvent("window");
      await page.getByRole("tab", { name: "Graph editor", exact: true }).click({ button: "right" });
      const tabMenu = page.getByRole("menu", { name: "Panel actions for Graph editor" });
      await expect(tabMenu).toBeVisible();
      await tabMenu.getByRole("menuitem", { name: /^Move to new window/ }).click();
      const popout = await opened;
      await popout.waitForLoadState("domcontentloaded");
      const pt = tools(popout);
      await expect(pt.panel).toBeVisible();
      await expect(pt.panel.getByRole("list", { name: "Curves" })).toContainText("Position");
      const pg = await settledGraph(popout);
      const there = at(pg, 4, 300);
      await popout.mouse.click(there.x, there.y);
      await expect(pt.time).toHaveValue("00:00:04:00");
      await expect(pt.value).toHaveValue("300");
      await popout.keyboard.press("ArrowUp");
      await expect.poll(() => xs(page, id)).toEqual([-300, 301, 100]);
      await expect(pt.value).toHaveValue("301");
      expect(await violations(popout, ".cw-graph-panel")).toEqual([]);
      await snapshot(popout, info, "graph editor detached");
      // Closing the window docks the panel back.
      const closed = popout.waitForEvent("close");
      await popout.close();
      await closed;
      await expect(tools(page).panel).toBeVisible();
      await shortcut(app, "Control+Z");
      await expect.poll(() => positionKeys(page, id)).toEqual(dragged);

      // 17. Closing the panel from View > Panels leaves the animation as it is.
      await toggleGraphPanel(page);
      await expect(tools(page).panel).toBeHidden();
      expect(await positionKeys(page, id)).toEqual(dragged);
    } finally {
      await close();
    }
  });
});
