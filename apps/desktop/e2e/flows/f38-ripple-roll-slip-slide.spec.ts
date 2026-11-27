// SPDX-License-Identifier: GPL-3.0-or-later
// F38 Ripple, roll, slip and slide (docs/user-flows.md, WP-2A.04, WP-2A.08, WP-2A.10). Three
// clips with sound butted on free tracks; with the ripple tool (B) an edge drag moves what
// follows by the same amount; the roll tool (toolbar) moves a cut and keeps the total length;
// the slip tool changes only which part of the media a clip shows; the slide tool moves a clip
// between its neighbours, which give and take; an end dragged past the media stops there; Esc in
// the middle of a slide puts everything back. The linked sounds follow every edit, and each drag
// is one undo step (Ctrl+Z). WP-2A.10's menu edit: Lock from V1's header menu, after which a
// ripple drag on it is refused with a toast; Unlock from the same menu. Real clicks, drags and
// keys; only the OS file picker is stubbed.
import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import {
  clipAt,
  createProject,
  drag,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  importWithButton,
  item,
  laneY,
  launchFlowApp,
  layout,
  sec,
  shortcut,
  showTimeline,
  snapshot,
  timelineCanvas,
  timelineGeometry,
  timelineTools,
  toast,
  toolbar,
  viewSettled,
  workspace,
  xAt,
} from "./helpers.ts";

/** V1's clips (and A1's, which follow them) as `[start, duration]` seconds. */
async function clips(page: Page): Promise<[number, number][]> {
  const tracks = await layout(page);
  const v1 = tracks.find((t) => t.track === "V1")?.clips ?? [];
  const a1 = tracks.find((t) => t.track === "A1")?.clips ?? [];
  expect(a1).toEqual(v1);
  return v1;
}

test.describe("F38 ripple, roll, slip and slide", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("each tool edits as it should, one undo step per drag; Esc puts a slide back; a locked track refuses", async () => {
    test.setTimeout(240_000);
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Trims", work.dir);
      await importWithButton(app, page, [
        work.file("interview.mp4"),
        work.file("broll-city.mp4"),
        work.file("broll-coffee.mp4"),
      ]);
      // Zoom out (the toolbar) until the three clips will show.
      await showTimeline(page, 31);
      await dropCard(page, "interview.mp4", "V1", 0);
      await expect.poll(() => clips(page)).toEqual([[0, 10]]);
      await editorCaughtUp(page);
      await dropCard(page, "broll-city.mp4", "V1", 10);
      await expect
        .poll(() => clips(page))
        .toEqual([
          [0, 10],
          [10, 10],
        ]);
      await editorCaughtUp(page);
      await dropCard(page, "broll-coffee.mp4", "V1", 20);
      await expect
        .poll(() => clips(page))
        .toEqual([
          [0, 10],
          [10, 10],
          [20, 10],
        ]);
      await editorCaughtUp(page);
      const tools = timelineTools(page);
      const tool = (name: string) => tools.getByRole("button", { name, exact: true });
      const undo = toolbar(page).getByRole("button", { name: /^Undo/ });
      const canvas = timelineCanvas(page).locator("canvas");
      await tool("Zoom to fit").click();
      await viewSettled(page);
      await expect.poll(async () => (await timelineGeometry(page)).pxPerSec).toBeLessThan(60);

      // B: the ripple tool. The interview's end 4 s to the left: what follows moves 4 s left.
      await canvas.focus();
      await page.keyboard.press("b");
      await expect(tool("Ripple")).toHaveAttribute("aria-pressed", "true");
      let g = await timelineGeometry(page);
      const y = laneY(g, "V1");
      let x = xAt(g, sec(10)) - 3;
      await drag(page, { x, y }, { x: x - 4 * g.pxPerSec, y });
      await expect
        .poll(() => clips(page))
        .toEqual([
          [0, 6],
          [6, 10],
          [16, 10],
        ]);
      await expect(undo).toHaveAccessibleName("Undo Ripple trim");
      await editorCaughtUp(page);
      // The B-roll's end 2 s to the left: the last clip follows.
      g = await timelineGeometry(page);
      x = xAt(g, sec(16)) - 3;
      await drag(page, { x, y }, { x: x - 2 * g.pxPerSec, y });
      await expect
        .poll(() => clips(page))
        .toEqual([
          [0, 6],
          [6, 8],
          [14, 10],
        ]);
      await editorCaughtUp(page);

      // The roll tool, from the toolbar: the cut at 6 s goes to 7 s, the total stays 24 s.
      await tool("Roll").click();
      g = await timelineGeometry(page);
      x = xAt(g, sec(6)) - 2;
      await drag(page, { x, y }, { x: x + g.pxPerSec, y });
      await expect
        .poll(() => clips(page))
        .toEqual([
          [0, 7],
          [7, 7],
          [14, 10],
        ]);
      await expect(undo).toHaveAccessibleName("Undo Roll edit");
      await editorCaughtUp(page);
      g = await timelineGeometry(page);
      const city = clipAt(g, "V1", 7).id;
      expect((await item(page, city)).source?.source_in).toBe(sec(1));

      // The slip tool: dragging the B-roll right shows earlier media; start and length stay.
      await tool("Slip").click();
      x = xAt(g, sec(10));
      await drag(page, { x, y }, { x: x + g.pxPerSec / 2, y });
      await expect.poll(async () => (await item(page, city)).source?.source_in).toBe(sec(0.5));
      expect(await clips(page)).toEqual([
        [0, 7],
        [7, 7],
        [14, 10],
      ]);
      await expect(undo).toHaveAccessibleName("Undo Slip clip");
      await editorCaughtUp(page);

      // The slide tool: the B-roll half a second right; its neighbours give and take.
      await tool("Slide").click();
      g = await timelineGeometry(page);
      x = xAt(g, sec(10));
      await drag(page, { x, y }, { x: x + g.pxPerSec / 2, y });
      await expect
        .poll(() => clips(page))
        .toEqual([
          [0, 7.5],
          [7.5, 7],
          [14.5, 9.5],
        ]);
      await expect(undo).toHaveAccessibleName("Undo Slide clip");
      await editorCaughtUp(page);
      await snapshot(page, info, "after ripple, roll, slip and slide");

      // Esc in the middle of a slide: everything goes back, and nothing new to undo.
      g = await timelineGeometry(page);
      x = xAt(g, sec(11));
      await page.mouse.move(x, y);
      await page.mouse.down();
      await page.mouse.move(x + 4, y, { steps: 2 });
      await page.mouse.move(x + g.pxPerSec, y, { steps: 8 });
      await expect.poll(async () => (await clips(page))[1]?.[0]).toBe(8.5);
      await page.keyboard.press("Escape");
      await page.mouse.up();
      await expect
        .poll(() => clips(page))
        .toEqual([
          [0, 7.5],
          [7.5, 7],
          [14.5, 9.5],
        ]);
      await expect(undo).toHaveAccessibleName("Undo Slide clip");
      await editorCaughtUp(page);

      // V, then the last clip's end dragged far past its media: it stops where the media ends.
      await canvas.focus();
      await page.keyboard.press("v");
      g = await timelineGeometry(page);
      x = xAt(g, sec(24)) - 3;
      await drag(page, { x, y }, { x: g.box.x + g.box.width - 6, y });
      await editorCaughtUp(page);
      expect((await clips(page))[2]).toEqual([14.5, 9.5]);

      // Ctrl+Z undoes the slide in one step.
      await shortcut(app, "Control+Z");
      await expect
        .poll(() => clips(page))
        .toEqual([
          [0, 7],
          [7, 7],
          [14, 10],
        ]);
      await editorCaughtUp(page);

      // V1's header menu: Lock. A ripple drag on it is then refused, with a toast; Unlock.
      await canvas.focus();
      await page.keyboard.press("b");
      g = await timelineGeometry(page);
      const header = { x: g.box.x + 12, y };
      await page.mouse.click(header.x, header.y, { button: "right" });
      const trackMenu = page.getByRole("menu", { name: "Track actions for V1" });
      await trackMenu.getByRole("menuitemcheckbox", { name: /^Lock/ }).click();
      await expect(page.getByRole("button", { name: "Unlock track V1" })).toBeVisible();
      x = xAt(g, sec(7)) - 3;
      await drag(page, { x, y }, { x: x - g.pxPerSec, y });
      await expect(toast(page, "Clip not moved")).toBeVisible();
      expect(await clips(page)).toEqual([
        [0, 7],
        [7, 7],
        [14, 10],
      ]);
      await page.mouse.click(header.x, header.y, { button: "right" });
      await trackMenu.getByRole("menuitemcheckbox", { name: /^Unlock/ }).click();
      await expect(page.getByRole("button", { name: "Lock track V1" })).toBeVisible();
    } finally {
      await close();
      work.remove();
    }
  });
});
