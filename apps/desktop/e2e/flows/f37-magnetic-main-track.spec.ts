// SPDX-License-Identifier: GPL-3.0-or-later
// F37 Magnetic main track (docs/user-flows.md, WP-2A.03, WP-2A.04, WP-2A.10; timeline spec
// §4.1-§4.3). V1's header menu makes it the magnetic main track; three clips with sound dropped
// on it butt from 0; a fourth dropped between the first two goes in at that cut and pushes the
// rest right; dragging the last one to the front reorders; Delete on the second and a drag of
// the first one's end left close the space; the clip menu's Ripple delete takes a clip out with
// no gap (Ctrl+Z puts it back, Ctrl+Y again); with the magnet off in the header menu a clip
// dragged right leaves a gap. The linked sounds on A1 follow every step, and each step is one
// undo. Real clicks, drags and keys; only the OS file picker is stubbed.
import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import {
  clipAt,
  clipBox,
  clipNames,
  createProject,
  drag,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  importWithButton,
  laneY,
  launchFlowApp,
  layout,
  sec,
  shortcut,
  snapshot,
  timelineGeometry,
  timelineTools,
  toolbar,
  workspace,
  xAt,
} from "./helpers.ts";

/** V1's clips as `[start, duration]` seconds; A1's sounds must match them. */
async function clips(page: Page): Promise<[number, number][]> {
  const tracks = await layout(page);
  const v1 = tracks.find((t) => t.track === "V1")?.clips ?? [];
  const a1 = tracks.find((t) => t.track === "A1")?.clips ?? [];
  expect(a1).toEqual(v1);
  return v1;
}

test.describe("F37 magnetic main track", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("drops insert at cuts, a drag reorders, deletes and trims close the space; off, a gap stays", async () => {
    test.setTimeout(240_000);
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Magnetic", work.dir);
      await importWithButton(app, page, [
        work.file("interview.mp4"),
        work.file("broll-city.mp4"),
        work.file("broll-coffee.mp4"),
      ]);
      const undo = toolbar(page).getByRole("button", { name: /^Undo/ });
      const menu = page.getByRole("menu");

      // V1's header menu: Magnetic main track.
      let g = await timelineGeometry(page);
      const header = { x: g.box.x + 12, y: laneY(g, "V1") };
      const magnet = async (to: boolean) => {
        await page.mouse.click(header.x, header.y, { button: "right" });
        const trackMenu = page.getByRole("menu", { name: "Track actions for V1" });
        const row = trackMenu.getByRole("menuitemcheckbox", { name: /^Magnetic main track/ });
        await expect(row).toBeChecked({ checked: !to });
        await row.click();
        await expect(menu).toHaveCount(0);
        // The menu shows the engine's answer: reopen it until the new state has come back.
        await expect(async () => {
          await page.mouse.click(header.x, header.y, { button: "right" });
          await expect(row).toBeChecked({ checked: to, timeout: 1500 });
        }).toPass({ timeout: 10_000 });
        await page.keyboard.press("Escape");
        await expect(menu).toHaveCount(0);
      };
      await magnet(true);

      // Three clips: wherever they are dropped they butt from 0, in the order they came.
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect.poll(() => clips(page)).toEqual([[0, 10]]);
      await editorCaughtUp(page);
      await expect(undo).toHaveAccessibleName("Undo Insert clip");
      await dropCard(page, "broll-city.mp4", "V1", 12);
      await expect
        .poll(() => clips(page))
        .toEqual([
          [0, 10],
          [10, 10],
        ]);
      await editorCaughtUp(page);
      await dropCard(page, "broll-coffee.mp4", "V1", 25);
      await expect
        .poll(() => clips(page))
        .toEqual([
          [0, 10],
          [10, 10],
          [20, 10],
        ]);
      await editorCaughtUp(page);
      await timelineTools(page).getByRole("button", { name: "Zoom to fit", exact: true }).click();
      await expect.poll(async () => (await timelineGeometry(page)).pxPerSec).toBeLessThan(60);

      // A fourth (the interview again) dropped just before the first cut goes in there and
      // pushes the rest right.
      await dropCard(page, "interview.mp4", "V1", 9);
      await expect
        .poll(() => clips(page))
        .toEqual([
          [0, 10],
          [10, 10],
          [20, 10],
          [30, 10],
        ]);
      await editorCaughtUp(page);
      expect(await clipNames(page, "V1")).toEqual([
        "interview.mp4",
        "interview.mp4",
        "broll-city.mp4",
        "broll-coffee.mp4",
      ]);

      // The last one dragged to the front: the clips reorder, still butted.
      await timelineTools(page).getByRole("button", { name: "Zoom to fit", exact: true }).click();
      g = await timelineGeometry(page);
      let box = clipBox(g, clipAt(g, "V1", 30).id);
      await drag(
        page,
        { x: box.x + 20, y: box.y },
        { x: g.box.x + g.headerWidth + 20 + 2, y: box.y },
      );
      await expect
        .poll(() => clipNames(page, "V1"))
        .toEqual(["broll-coffee.mp4", "interview.mp4", "interview.mp4", "broll-city.mp4"]);
      expect(await clips(page)).toEqual([
        [0, 10],
        [10, 10],
        [20, 10],
        [30, 10],
      ]);
      await expect(undo).toHaveAccessibleName("Undo Move clip");
      await editorCaughtUp(page);

      // Delete on the second clip: the space closes.
      g = await timelineGeometry(page);
      box = clipBox(g, clipAt(g, "V1", 10).id);
      await page.mouse.click(box.x + 20, box.y);
      await page.keyboard.press("Delete");
      await expect
        .poll(() => clips(page))
        .toEqual([
          [0, 10],
          [10, 10],
          [20, 10],
        ]);
      await editorCaughtUp(page);

      // The first clip's end dragged 4 s left: the rest follows.
      g = await timelineGeometry(page);
      const y = laneY(g, "V1");
      const end = xAt(g, sec(10)) - 3;
      await drag(page, { x: end, y }, { x: end - 4 * g.pxPerSec, y });
      await expect
        .poll(() => clips(page))
        .toEqual([
          [0, 6],
          [6, 10],
          [16, 10],
        ]);
      await expect(undo).toHaveAccessibleName("Undo Trim clip");
      await editorCaughtUp(page);
      await snapshot(page, info, "main track after the edits");

      // The clip menu's Ripple delete on the middle clip: no gap; Ctrl+Z, Ctrl+Y.
      g = await timelineGeometry(page);
      box = clipBox(g, clipAt(g, "V1", 6).id);
      await page.mouse.click(box.x + 20, box.y, { button: "right" });
      const clipMenu = page.getByRole("menu", { name: /^Clip actions for/ });
      await clipMenu.getByRole("menuitem", { name: /^Ripple delete/ }).click();
      await expect
        .poll(() => clips(page))
        .toEqual([
          [0, 6],
          [6, 10],
        ]);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect
        .poll(() => clips(page))
        .toEqual([
          [0, 6],
          [6, 10],
          [16, 10],
        ]);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Y");
      await expect
        .poll(() => clips(page))
        .toEqual([
          [0, 6],
          [6, 10],
        ]);
      await editorCaughtUp(page);

      // The magnet off (header menu): a clip dragged 2 s right leaves a gap.
      await magnet(false);
      g = await timelineGeometry(page);
      box = clipBox(g, clipAt(g, "V1", 6).id);
      await drag(page, { x: box.x + 20, y: box.y }, { x: box.x + 20 + 2 * g.pxPerSec, y: box.y });
      await expect
        .poll(() => clips(page))
        .toEqual([
          [0, 6],
          [8, 10],
        ]);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect
        .poll(() => clips(page))
        .toEqual([
          [0, 6],
          [6, 10],
        ]);
    } finally {
      await close();
      work.remove();
    }
  });
});
