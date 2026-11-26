// SPDX-License-Identifier: GPL-3.0-or-later
// F36 Two videos side by side (docs/user-flows.md, WP-2C.08): select two videos in the bin, choose
// "Place side by side" from the card menu; both land at the playhead on two tracks and the preview
// overlay shows each clip's picture hugging its half. Swap in the Inspector exchanges them;
// Ctrl+Z takes the swap back and
// Ctrl+Z again takes the placing back (the inserts and the layout are one step). Then select both
// clips on the timeline and pick Picture in picture in the Inspector: the second clip sits small in
// a corner. Real clicks and keys; only the OS file picker is stubbed. The maths (every layout on
// every canvas) are Rust unit tests; the golden frame is `side_by_side`.
import type { Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import {
  clickClip,
  clipAt,
  clipBox,
  createProject,
  editorCaughtUp,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  layout,
  mediaCard,
  sequence,
  shortcut,
  snapshot,
  timelineGeometry,
  workspace,
} from "./helpers.ts";

interface Placed {
  visual: {
    transform: { position: { x: number; y: number }; scale: { x: number; y: number } };
    crop: { left: number; right: number };
  };
}

const placed = (page: Page, id: string) => editorCall<Placed>(page, "item.get", { item: id });

/** The box of the selected clip's handles over the preview, as fractions of the preview frame. */
async function overlayFractions(page: Page) {
  const frame = await page.locator("canvas.cw-preview__canvas").boundingBox();
  const box = await page.locator(".cw-xform__outline").first().boundingBox();
  if (frame === null || box === null) throw new Error("no preview frame or overlay");
  return {
    left: (box.x - frame.x) / frame.width,
    right: (box.x + box.width - frame.x) / frame.width,
    top: (box.y - frame.y) / frame.height,
    bottom: (box.y + box.height - frame.y) / frame.height,
  };
}

test.describe("F36 side by side", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("place two videos side by side, swap, undo, and picture in picture", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Layouts", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4"), work.file("broll-city.mp4")]);
      await editorCaughtUp(page);

      // 1. Select both cards and place them side by side from the card's menu.
      await mediaCard(page, "interview.mp4").click();
      await mediaCard(page, "broll-city.mp4").click({ modifiers: ["Control"] });
      await mediaCard(page, "broll-city.mp4").click({ button: "right" });
      const menu = page.getByRole("menu");
      await expect(menu).toBeVisible();
      await snapshot(page, info, "bin card menu with two videos");
      await menu.getByRole("menuitem", { name: "Place side by side" }).click();
      await expect(menu).toHaveCount(0);
      await expect
        .poll(async () =>
          (await layout(page)).filter((t) => t.track.startsWith("V") && t.clips.length > 0),
        )
        .toEqual([
          { track: "V2", clips: [[0, 10]] },
          { track: "V1", clips: [[0, 10]] },
        ]);
      await editorCaughtUp(page);
      const g = await timelineGeometry(page);
      const interview = clipAt(g, "V1", 0).id;
      const city = clipAt(g, "V2", 0).id;
      // The first video fills the left half, the second the right half.
      await expect
        .poll(async () => (await placed(page, interview)).visual.transform.position.x)
        .toBeCloseTo(-0.25, 3);
      expect((await placed(page, city)).visual.transform.position.x).toBeCloseTo(0.25, 3);
      for (const id of [interview, city]) {
        const p = await placed(page, id);
        expect(p.visual.crop.left).toBeCloseTo(p.visual.crop.right, 6);
        expect(p.visual.transform.position.y).toBeCloseTo(0, 6);
      }
      // On screen the first video's handles hug the left half of the preview, the second's the right.
      await clickClip(page, interview);
      await expect.poll(async () => (await overlayFractions(page)).right).toBeCloseTo(0.5, 1);
      expect((await overlayFractions(page)).left).toBeCloseTo(0, 1);
      await clickClip(page, city);
      await expect.poll(async () => (await overlayFractions(page)).left).toBeCloseTo(0.5, 1);
      expect((await overlayFractions(page)).right).toBeCloseTo(1, 1);
      // Both together again for the Inspector's Layout row.
      const interviewBox = clipBox(await timelineGeometry(page), interview);
      await page.keyboard.down("Shift");
      await page.mouse.click(interviewBox.x + Math.min(interviewBox.w / 2, 60), interviewBox.y);
      await page.keyboard.up("Shift");
      await snapshot(page, info, "side by side");

      // 2. Swap, in the Inspector (both clips are selected after placing).
      const layoutSection = page.getByTestId("layout-section");
      await expect(layoutSection).toBeVisible();
      await snapshot(page, info, "inspector layout row");
      await layoutSection.getByRole("button", { name: "Swap" }).click();
      await expect
        .poll(async () => (await placed(page, interview)).visual.transform.position.x)
        .toBeCloseTo(0.25, 3);
      expect((await placed(page, city)).visual.transform.position.x).toBeCloseTo(-0.25, 3);

      // 3. Ctrl+Z takes the swap back; Ctrl+Z again takes the placing back, inserts and layout.
      await shortcut(app, "Control+Z");
      await expect
        .poll(async () => (await placed(page, interview)).visual.transform.position.x)
        .toBeCloseTo(-0.25, 3);
      await shortcut(app, "Control+Z");
      await expect
        .poll(async () => (await sequence(page)).tracks.reduce((n, t) => n + t.items.length, 0))
        .toBe(0);
      await editorCaughtUp(page);

      // 4. Redo brings both back; select both on the timeline and pick Picture in picture.
      await shortcut(app, "Control+Y");
      await expect
        .poll(async () => (await sequence(page)).tracks.reduce((n, t) => n + t.items.length, 0))
        .toBe(4);
      await editorCaughtUp(page);
      const again = await timelineGeometry(page);
      const first = clipAt(again, "V2", 0).id;
      const second = clipAt(again, "V1", 0).id;
      await clickClip(page, first);
      const box = clipBox(await timelineGeometry(page), second);
      await page.keyboard.down("Shift");
      await page.mouse.click(box.x + Math.min(box.w / 2, 60), box.y);
      await page.keyboard.up("Shift");
      const pickers = page.getByRole("group", { name: "Choose a layout" });
      await expect(pickers).toBeVisible();
      await pickers.getByRole("button", { name: "Picture in picture, top right" }).click();
      // The clip on the top track sits small in the top right corner, the other fills the frame.
      await expect
        .poll(async () => (await placed(page, first)).visual.transform.scale.x)
        .toBeCloseTo(0.3, 3);
      const small = await placed(page, first);
      expect(small.visual.transform.position.x).toBeCloseTo(0.3, 3);
      expect(small.visual.transform.position.y).toBeCloseTo(-0.3, 3);
      const big = await placed(page, second);
      expect(big.visual.transform.scale.x).toBeCloseTo(1, 3);
      expect(big.visual.transform.position.x).toBeCloseTo(0, 3);
      await snapshot(page, info, "picture in picture");
      // One undo puts the layout back.
      await shortcut(app, "Control+Z");
      await expect
        .poll(async () => (await placed(page, first)).visual.transform.scale.x)
        .toBeCloseTo(1, 3);
    } finally {
      await close();
      work.remove();
    }
  });
});
