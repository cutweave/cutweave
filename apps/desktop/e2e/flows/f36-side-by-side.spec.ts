// SPDX-License-Identifier: GPL-3.0-or-later
// F36 Two videos side by side (docs/user-flows.md, WP-2C.08): select two videos in the bin, choose
// "Place side by side" from the card menu; both land at the playhead on two tracks and the preview
// shows a picture in each half. Swap in the Inspector exchanges them; Ctrl+Z takes the swap back and
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

/** The colour at a point of the preview canvas, as fractions of its size (decoded in the page). */
async function previewPixel(page: Page, fx: number, fy: number): Promise<string> {
  const box = await page.locator("canvas.cw-preview__canvas").boundingBox();
  if (box === null) throw new Error("no preview canvas");
  const png = await page.screenshot({
    clip: {
      x: Math.round(box.x + box.width * fx),
      y: Math.round(box.y + box.height * fy),
      width: 4,
      height: 4,
    },
    animations: "allow",
    caret: "initial",
  });
  return page.evaluate(async (b64) => {
    const blob = await (await fetch(`data:image/png;base64,${b64}`)).blob();
    const bitmap = await createImageBitmap(blob);
    const canvas = document.createElement("canvas");
    canvas.width = bitmap.width;
    canvas.height = bitmap.height;
    const ctx = canvas.getContext("2d");
    if (ctx === null) throw new Error("no 2d context");
    ctx.drawImage(bitmap, 0, 0);
    const [r, g, b] = ctx.getImageData(1, 1, 1, 1).data;
    return `${r},${g},${b}`;
  }, png.toString("base64"));
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
      const nothing = await previewPixel(page, 0.25, 0.25);

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
        .poll(async () => (await layout(page)).filter((t) => t.clips.length > 0))
        .toEqual([
          { track: "V2", clips: [[0, 10]] },
          { track: "V1", clips: [[0, 10]] },
        ]);
      await editorCaughtUp(page);
      const g = await timelineGeometry(page);
      const interview = clipAt(g, "V1", 0).id;
      const city = clipAt(g, "V2", 0).id;
      // The first video fills the left half, the second the right half.
      await expect.poll(async () => (await placed(page, interview)).visual.transform.position.x)
        .toBeCloseTo(-0.25, 3);
      expect((await placed(page, city)).visual.transform.position.x).toBeCloseTo(0.25, 3);
      for (const id of [interview, city]) {
        const p = await placed(page, id);
        expect(p.visual.crop.left).toBeCloseTo(p.visual.crop.right, 6);
        expect(p.visual.transform.position.y).toBeCloseTo(0, 6);
      }
      // The preview shows a picture in the left and the right half, edge to edge: no bars.
      for (const [fx, fy] of [
        [0.03, 0.25],
        [0.47, 0.25],
        [0.53, 0.25],
        [0.97, 0.25],
      ] as const) {
        await expect.poll(() => previewPixel(page, fx, fy)).not.toBe(nothing);
      }
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
        .poll(async () =>
          (await sequence(page)).tracks.reduce((n, t) => n + t.items.length, 0),
        )
        .toBe(0);
      await editorCaughtUp(page);

      // 4. Redo brings both back; select both on the timeline and pick Picture in picture.
      await shortcut(app, "Control+Y");
      await expect
        .poll(async () =>
          (await sequence(page)).tracks.reduce((n, t) => n + t.items.length, 0),
        )
        .toBe(2);
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
      await expect.poll(async () => (await placed(page, first)).visual.transform.scale.x)
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
