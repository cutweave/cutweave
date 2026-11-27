// SPDX-License-Identifier: GPL-3.0-or-later
// F64 Edit a marker and give a clip a colour label (docs/user-flows.md, issue #222): M adds a
// marker; a double-click on its pin opens Edit marker, where it gets a name, a colour and a
// length; a drag along the ruler moves it (one undo step, snapping to a clip's end); its menu
// makes it a chapter; the clip's menu gives the clip and its sound a colour label. Each step is
// undone with Ctrl+Z and done again with Ctrl+Y, and everything is there after save, quit and
// reopen. Real clicks, drags and keys; only the OS file picker is stubbed.
import type { Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import {
  chooseMenu,
  clipAt,
  clipBox,
  createProject,
  dirtyDot,
  drag,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  layout,
  movePlayhead,
  openRecent,
  profileDir,
  removeDir,
  sec,
  sequence,
  shortcut,
  showTimeline,
  snapshot,
  type TimelineGeometry,
  timelineCanvas,
  timelineGeometry,
  timelineTools,
  toolbar,
  viewSettled,
  workspace,
  xAt,
} from "./helpers.ts";

interface MarkerView {
  readonly t: number;
  readonly duration: number | null;
  readonly label: string;
  readonly color: string;
  readonly kind: string;
}

/** The sequence's markers as the engine has them (ids left out). */
async function markers(page: Page): Promise<MarkerView[]> {
  const out = await editorCall<{ markers: (MarkerView & { id: string })[] }>(
    page,
    "timeline.list_markers",
    { sequence: (await sequence(page)).id },
  );
  return out.markers.map(({ t, duration, label, color, kind }) => ({
    t,
    duration: duration ?? null,
    label,
    color,
    kind,
  }));
}

/** The colour labels of the clips on `trackName`, as the engine has them (`null`: none). */
async function labels(page: Page, trackName: string): Promise<(string | null)[]> {
  const track = (await sequence(page)).tracks.find((t) => t.name === trackName);
  return (track?.items ?? []).map(
    (i) => (i as unknown as { color_label?: string }).color_label ?? null,
  );
}

/** A label colour's value, read from the design tokens of the page (never a literal here). */
const token = (page: Page, name: string): Promise<string> =>
  page.evaluate(
    (n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim().toLowerCase(),
    `--label-${name}`,
  );

/** The page point of a marker's pin at time `t` (the pins sit at the bottom of the ruler). */
const pin = (g: TimelineGeometry, t: number) => ({
  x: xAt(g, t),
  y: g.box.y + g.rulerHeight - 3,
});

test.describe("F64 edit a marker and give a clip a colour label", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("add, edit by double-click, drag, convert to chapter, colour label, undo each, save and reopen", async () => {
    test.setTimeout(300_000);
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    try {
      const first = await launchFlowApp(work, home);
      const { app, page } = first;
      await createProject(app, page, "Markers", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect
        .poll(() => layout(page))
        .toEqual([
          { track: "V1", clips: [[1, 10]] },
          { track: "A1", clips: [[1, 10]] },
        ]);
      await editorCaughtUp(page);
      await showTimeline(page, 24);
      const undo = toolbar(page).getByRole("button", { name: /^Undo/ });
      const canvas = timelineCanvas(page).locator("canvas");
      const blue = await token(page, "blue");
      const red = await token(page, "red");
      expect(blue).toMatch(/^#[0-9a-f]{6}$/);
      expect(red).toMatch(/^#[0-9a-f]{6}$/);

      // 1. M adds a marker at the playhead (5 s).
      await movePlayhead(page, 5);
      await canvas.focus();
      await page.keyboard.press("m");
      await expect.poll(async () => (await markers(page)).map((m) => m.t)).toEqual([sec(5)]);
      const plain = (await markers(page))[0];
      if (plain === undefined) throw new Error("no marker");
      expect(plain).toMatchObject({ label: "", kind: "user", duration: null });

      // 2. A double-click on its pin opens Edit marker: a name, a colour and a length.
      let g = await timelineGeometry(page);
      let at = pin(g, sec(5));
      await page.mouse.dblclick(at.x, at.y);
      const dialog = page.getByRole("dialog", { name: "Edit marker" });
      await expect(dialog).toBeVisible();
      const viewport = await page.evaluate(() => window.innerHeight);
      const size = await dialog.boundingBox();
      expect(size?.height ?? Number.POSITIVE_INFINITY).toBeLessThanOrEqual(0.6 * viewport + 1);
      await dialog.getByRole("textbox", { name: "Marker name" }).fill("Intro");
      await dialog.getByRole("radio", { name: "Blue" }).check();
      await dialog.getByRole("textbox", { name: "Length in seconds" }).fill("2");
      await snapshot(page, info, "edit marker dialog");
      await dialog.getByRole("button", { name: "Save" }).click();
      await expect(dialog).toBeHidden();
      const edited = { t: sec(5), duration: sec(2), label: "Intro", color: blue, kind: "user" };
      await expect.poll(() => markers(page)).toEqual([edited]);
      await expect(undo).toHaveAccessibleName("Undo Edit marker");
      // One step: Ctrl+Z puts the bare marker back, Ctrl+Y the edit.
      await shortcut(app, "Control+Z");
      await expect.poll(() => markers(page)).toEqual([plain]);
      await shortcut(app, "Control+Y");
      await expect.poll(() => markers(page)).toEqual([edited]);

      // 3. Drag it along the ruler to the playhead at 14 s (it snaps onto it): it follows, and
      // the drag is one undo step.
      await movePlayhead(page, 14);
      g = await timelineGeometry(page);
      at = pin(g, sec(5));
      await drag(page, at, { x: xAt(g, sec(14)), y: at.y });
      await expect.poll(async () => (await markers(page))[0]?.t).toBe(sec(14));
      await expect(undo).toHaveAccessibleName("Undo Move marker");
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await markers(page))[0]?.t).toBe(sec(5));
      await shortcut(app, "Control+Y");
      await expect.poll(async () => (await markers(page))[0]?.t).toBe(sec(14));
      // Dropped a few pixels short of the clip's end (11 s), it snaps onto it.
      g = await timelineGeometry(page);
      at = pin(g, sec(14));
      await drag(page, at, { x: xAt(g, sec(11)) + 4, y: at.y });
      await expect.poll(async () => (await markers(page))[0]?.t).toBe(sec(11));
      // Esc in the middle of a drag puts it back, with nothing new to undo.
      await movePlayhead(page, 16);
      g = await timelineGeometry(page);
      at = pin(g, sec(11));
      await page.mouse.move(at.x, at.y);
      await page.mouse.down();
      await page.mouse.move(at.x + 4, at.y, { steps: 2 });
      await page.mouse.move(xAt(g, sec(16)), at.y, { steps: 8 });
      await expect.poll(async () => (await markers(page))[0]?.t).toBe(sec(16));
      await page.keyboard.press("Escape");
      await page.mouse.up();
      await expect.poll(async () => (await markers(page))[0]?.t).toBe(sec(11));
      await expect(undo).toHaveAccessibleName("Undo Move marker");
      await snapshot(page, info, "marker moved");

      // 4. Its menu makes it a chapter (and offers the way back).
      const markerMenu = page.getByRole("menu", { name: "Marker actions for Intro" });
      const openMarkerMenu = async () => {
        g = await timelineGeometry(page);
        at = pin(g, sec(11));
        await page.mouse.click(at.x, at.y, { button: "right" });
        await expect(markerMenu).toBeVisible();
      };
      await openMarkerMenu();
      await markerMenu.getByRole("menuitem", { name: "Convert to chapter" }).click();
      await expect.poll(async () => (await markers(page))[0]?.kind).toBe("chapter");
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await markers(page))[0]?.kind).toBe("user");
      await shortcut(app, "Control+Y");
      await expect.poll(async () => (await markers(page))[0]?.kind).toBe("chapter");
      await openMarkerMenu();
      await expect(markerMenu.getByRole("menuitem", { name: "Convert to marker" })).toBeVisible();
      // Edit… in the menu opens the same dialog, filled in with what the marker has.
      await markerMenu.getByRole("menuitem", { name: /^Edit/ }).click();
      await expect(dialog).toBeVisible();
      await expect(dialog.getByRole("textbox", { name: "Marker name" })).toHaveValue("Intro");
      await expect(dialog.getByRole("radio", { name: "Blue" })).toBeChecked();
      await expect(dialog.getByRole("textbox", { name: "Length in seconds" })).toHaveValue("2");
      await dialog.getByRole("button", { name: "Cancel" }).click();
      await expect(dialog).toBeHidden();

      // 5. The clip's menu gives it a colour label: the picture and its sound.
      g = await timelineGeometry(page);
      const box = clipBox(g, clipAt(g, "V1", 1).id);
      const clipMenu = page.getByRole("menu", { name: "Clip actions for interview.mp4" });
      const labelMenu = page.getByRole("menu", { name: "Color label" });
      const openLabelMenu = async () => {
        await page.mouse.click(box.x + 40, box.y, { button: "right" });
        await expect(clipMenu).toBeVisible();
        await clipMenu.getByRole("menuitem", { name: /^Color label/ }).click();
        await expect(labelMenu).toBeVisible();
      };
      await openLabelMenu();
      await expect(labelMenu.getByRole("menuitemradio", { name: "None" })).toBeChecked();
      await labelMenu.getByRole("menuitemradio", { name: "Red" }).click();
      await expect.poll(() => labels(page, "V1")).toEqual([red]);
      await expect.poll(() => labels(page, "A1")).toEqual([red]);
      await expect(undo).toHaveAccessibleName("Undo Set color label");
      await shortcut(app, "Control+Z");
      await expect.poll(() => labels(page, "V1")).toEqual([null]);
      await expect.poll(() => labels(page, "A1")).toEqual([null]);
      await shortcut(app, "Control+Y");
      await expect.poll(() => labels(page, "V1")).toEqual([red]);
      await openLabelMenu();
      await expect(labelMenu.getByRole("menuitemradio", { name: "Red" })).toBeChecked();
      await page.keyboard.press("Escape");
      await page.keyboard.press("Escape");
      await expect(page.getByRole("menu")).toHaveCount(0);
      await snapshot(page, info, "chapter and colour label");

      // 6. Save, quit and reopen: the marker and the label are in the project file.
      const kept = { t: sec(11), duration: sec(2), label: "Intro", color: blue, kind: "chapter" };
      expect(await markers(page)).toEqual([kept]);
      await shortcut(app, "Control+S");
      await expect(dirtyDot(page)).toBeHidden();
      const closed = app.waitForEvent("close");
      await chooseMenu(page, app, "File", "Exit");
      await closed;
      await first.close();

      const second = await launchFlowApp(work, home);
      try {
        const again = second.page;
        await openRecent(again, "Markers.cutweave");
        await expect(again).toHaveTitle("Markers — Cutweave");
        await expect.poll(() => markers(again)).toEqual([kept]);
        await expect.poll(() => labels(again, "V1")).toEqual([red]);
        await expect.poll(() => labels(again, "A1")).toEqual([red]);
        await editorCaughtUp(again);
        // Zoom out once, as a person does to see the marker past the clip: from here the
        // engine's view and the canvas agree on the zoom (`timelineGeometry` reads the view).
        const zoomOut = timelineTools(again).getByRole("button", { name: "Zoom out", exact: true });
        await zoomOut.click();
        await viewSettled(again);
        await showTimeline(again, 14);
        await viewSettled(again);
        const g2 = await timelineGeometry(again);
        const at2 = pin(g2, sec(11));
        await snapshot(again, info, "reopened");
        await again.mouse.dblclick(at2.x, at2.y);
        const reopened = again.getByRole("dialog", { name: "Edit marker" });
        await expect(reopened).toBeVisible();
        await expect(reopened.getByRole("textbox", { name: "Marker name" })).toHaveValue("Intro");
        await expect(reopened.getByRole("radio", { name: "Blue" })).toBeChecked();
        await reopened.getByRole("button", { name: "Cancel" }).click();
        await expect(reopened).toBeHidden();
      } finally {
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
