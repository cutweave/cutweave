// SPDX-License-Identifier: GPL-3.0-or-later
// F45 Move, scale, rotate and crop in the preview (docs/user-flows.md, WP-2H.06): select a clip, drag
// its picture and see it move (the clip's position follows the pointer), undo; turn it by the
// rotate handle; crop by dragging an edge in crop mode; scale by a corner; reset it from the
// picture's own menu; nudge it with the arrow keys; turn on the safe areas, the thirds and a
// platform's zone. Real pointer drags, clicks and keys; nothing is stubbed. The clip's picture is
// the canvas-sized stand-in of the stub media, so a drag's distance in canvas fractions is its
// distance on screen over the overlay's size.
import type { Locator, Page } from "@playwright/test";
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
  launchFlowApp,
  layout,
  rulerY,
  sec,
  shortcut,
  snapshot,
  timecode,
  timelineGeometry,
  toolbar,
  workspace,
  xAt,
} from "./helpers.ts";

interface Placed {
  visual: {
    transform: {
      position: { x: number; y: number };
      scale: { x: number; y: number };
      rotation: number;
    };
    crop: { left: number; top: number; right: number; bottom: number };
  };
}

const placed = (page: Page, id: string) => editorCall<Placed>(page, "item.get", { item: id });

/** The centre of an element on screen. */
async function centre(locator: Locator): Promise<{ x: number; y: number }> {
  const box = await locator.boundingBox();
  if (box === null) throw new Error("element not on screen");
  return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
}

test.describe("F45 preview handles", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("drag, rotate, crop, scale, reset and nudge the picture, and show the guides", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await freeMainTrack(app, page); // clips stay where they are dropped
      await createProject(app, page, "Handles", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
      await editorCaughtUp(page);
      const g = await timelineGeometry(page);
      const id = clipAt(g, "V1", 1).id;
      const sequenceInfo = await editorCall<{
        sequences: { format: { width: number; height: number } }[];
      }>(page, "project.info");
      const canvas = sequenceInfo.sequences[0]?.format;
      if (canvas === undefined) throw new Error("no sequence format");

      // Put the playhead inside the clip (click the ruler at 3 s) and select the clip.
      await page.mouse.click(xAt(g, sec(3)), rulerY(g));
      await expect(timecode(page)).toHaveText(/^00:00:03/);
      await clickClip(page, id);
      const box = page.locator(".cw-xform");
      await expect(box).toBeVisible();
      await expect(page.locator('.cw-xform__handle[data-kind="scale"]')).toHaveCount(8);
      const frame = await box.boundingBox();
      if (frame === null) throw new Error("no overlay box");
      const pointOnPicture = {
        x: frame.x + frame.width * 0.3,
        y: frame.y + frame.height * 0.65,
      };
      const undo = toolbar(page).getByRole("button", { name: /^Undo/ });

      // 1. Drag the picture 80 px right and 40 px down: the clip's position follows the pointer.
      await drag(page, pointOnPicture, { x: pointOnPicture.x + 80, y: pointOnPicture.y + 40 });
      await expect
        .poll(async () => (await placed(page, id)).visual.transform.position.x)
        .toBeCloseTo(80 / frame.width, 2);
      expect((await placed(page, id)).visual.transform.position.y).toBeCloseTo(
        40 / frame.height,
        2,
      );
      await expect(undo).toHaveAccessibleName("Undo Move clip in the picture");
      // One undo puts it back (one drag is one step).
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await placed(page, id)).visual.transform.position.x).toBe(0);
      expect((await placed(page, id)).visual.transform.position.y).toBe(0);

      // 2. Rotate by the handle: 15 degrees about the anchor, then undo.
      const handle = await centre(page.locator('.cw-xform__handle[data-handle="rotate"]'));
      const anchor = await centre(page.locator(".cw-xform__anchor"));
      const radius = Math.hypot(handle.x - anchor.x, handle.y - anchor.y);
      const start = Math.atan2(handle.y - anchor.y, handle.x - anchor.x);
      const to = start + (15 * Math.PI) / 180;
      await drag(page, handle, {
        x: anchor.x + radius * Math.cos(to),
        y: anchor.y + radius * Math.sin(to),
      });
      await expect
        .poll(async () => Math.abs((await placed(page, id)).visual.transform.rotation - 15))
        .toBeLessThan(0.5);
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await placed(page, id)).visual.transform.rotation).toBe(0);

      // 3. Crop mode: drag the left edge a quarter of the way in; Escape leaves crop mode.
      const cropButton = page.getByRole("button", { name: "Crop with the handles" });
      await cropButton.click();
      await expect(cropButton).toHaveAttribute("aria-pressed", "true");
      await expect(page.locator('.cw-xform__handle[data-kind="crop"]')).toHaveCount(8);
      const edge = await centre(page.locator('.cw-xform__handle[data-handle="l"]'));
      await drag(page, edge, { x: edge.x + frame.width / 4, y: edge.y });
      await expect.poll(async () => (await placed(page, id)).visual.crop.left).toBeCloseTo(0.25, 1);
      // The picture stays where it is: only the crop changed.
      expect((await placed(page, id)).visual.transform.position.x).toBe(0);
      await page.keyboard.press("Escape");
      await expect(cropButton).toHaveAttribute("aria-pressed", "false");
      await expect(page.locator('.cw-xform__handle[data-kind="scale"]')).toHaveCount(8);
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await placed(page, id)).visual.crop.left).toBe(0);

      // 4. Scale by the bottom-right corner, to the picture's centre: half size.
      const corner = await centre(page.locator('.cw-xform__handle[data-handle="br"]'));
      await drag(page, corner, { x: frame.x + frame.width / 2, y: frame.y + frame.height / 2 });
      await expect
        .poll(async () => (await placed(page, id)).visual.transform.scale.x)
        .toBeCloseTo(0.5, 1);

      // 5. Reset from the picture's own menu, then take the reset back.
      // The picture is the top-left quarter now: right-click inside it.
      await page.mouse.click(frame.x + frame.width * 0.2, frame.y + frame.height * 0.2, {
        button: "right",
      });
      await page.getByRole("menuitem", { name: "Reset transform" }).click();
      await expect.poll(async () => (await placed(page, id)).visual.transform.scale.x).toBe(1);
      await shortcut(app, "Control+Z");
      await expect
        .poll(async () => (await placed(page, id)).visual.transform.scale.x)
        .toBeCloseTo(0.5, 1);

      // 6. Arrow keys nudge by one sequence pixel, ten with Shift, without stepping the playhead.
      const before = await placed(page, id);
      const time = (await timecode(page).textContent()) ?? "";
      const half = await centre(page.locator(".cw-xform__body"));
      await page.mouse.click(half.x + 20, half.y + 20);
      await page.keyboard.press("ArrowRight");
      await expect
        .poll(async () => (await placed(page, id)).visual.transform.position.x)
        .toBeCloseTo(before.visual.transform.position.x + 1 / canvas.width, 6);
      await page.keyboard.press("Shift+ArrowDown");
      await expect
        .poll(async () => (await placed(page, id)).visual.transform.position.y)
        .toBeCloseTo(before.visual.transform.position.y + 10 / canvas.height, 6);
      await expect(timecode(page)).toHaveText(time);
      await expect(undo).toHaveAccessibleName("Undo Nudge clip");

      // 7. Guides: the safe areas, the thirds and a platform's zone.
      const safe = page.getByRole("button", { name: "Safe areas" });
      await safe.click();
      await expect(page.locator('.cw-preview__safe[data-area="title"]')).toBeVisible();
      await page.getByRole("button", { name: "Guides" }).click();
      await page.getByRole("menu").getByText("Rule of thirds").click();
      await expect(page.locator('.cw-preview__line[data-axis="x"]')).toHaveCount(2);
      await page.getByRole("menu").getByText("TikTok").click();
      await expect(page.locator('.cw-preview__safe--platform[data-area="tiktok"]')).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(page.getByRole("menu")).toBeHidden();
      await snapshot(page, info, "preview guides");
      await safe.click();
      await expect(page.locator('.cw-preview__safe[data-area="title"]')).toHaveCount(0);
    } finally {
      await close();
      work.remove();
    }
  });
});
