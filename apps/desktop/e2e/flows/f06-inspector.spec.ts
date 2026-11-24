// SPDX-License-Identifier: GPL-3.0-or-later
// F6 Inspector (docs/user-flows.md): select a clip; edit its name, start and duration (typed
// timecodes), opacity, position, scale, rotation and gain, by typing, dragging and keys. The
// timeline follows timing changes, and input that is not a time is refused with a message.
import { expect, test } from "../fixtures.ts";
import {
  clickClip,
  clipAt,
  createProject,
  dropCard,
  editorCaughtUp,
  FLICKS,
  hasRealEngine,
  importWithButton,
  inspector,
  item,
  launchFlowApp,
  layout,
  snapshot,
  timelineGeometry,
  workspace,
} from "./helpers.ts";

test.describe("F6 inspector", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("typing, dragging and keys in the inspector edit the selected clip", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Inspector", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
      await editorCaughtUp(page);
      const id = clipAt(await timelineGeometry(page), "V1", 1).id;
      await clickClip(page, id);
      const f = inspector(page);
      await expect(f.start).toHaveValue("00:00:01:00");
      await expect(f.duration).toHaveValue("00:00:10:00");

      // Name: typed, Enter commits; Escape puts the old text back.
      await f.name.click();
      await page.keyboard.type("Opening shot");
      await page.keyboard.press("Enter");
      await expect.poll(async () => (await item(page, id)).name).toBe("Opening shot");
      await f.name.fill("Nope");
      await page.keyboard.press("Escape");
      await expect(f.name).toHaveValue("Opening shot");

      // Start as a timecode: the clip and its audio move.
      await f.start.fill("00:00:02:00");
      await page.keyboard.press("Enter");
      await expect
        .poll(() => layout(page))
        .toEqual([
          { track: "V1", clips: [[2, 10]] },
          { track: "A1", clips: [[2, 10]] },
        ]);
      await expect(f.start).toHaveValue("00:00:02:00");

      // Duration in seconds: the end edge moves.
      await f.duration.fill("4s");
      await page.keyboard.press("Enter");
      await expect
        .poll(() => layout(page))
        .toEqual([
          { track: "V1", clips: [[2, 4]] },
          { track: "A1", clips: [[2, 4]] },
        ]);
      await expect(f.duration).toHaveValue("00:00:04:00");

      // Not a time: a message, nothing changes; Escape takes the text back.
      await f.duration.fill("soon");
      await page.keyboard.press("Enter");
      await expect(f.panel.getByRole("alert")).toHaveText(
        "Not a time. Try 00:00:02:00, 2.5s or f:60.",
      );
      await expect(f.duration).toHaveAttribute("aria-invalid", "true");
      expect((await item(page, id)).item.duration).toBe(4 * FLICKS);
      await page.keyboard.press("Escape");
      await expect(f.panel.getByRole("alert")).toBeHidden();
      await expect(f.duration).toHaveValue("00:00:04:00");
      // A frame count works as a start too.
      await f.start.fill("f:45");
      await page.keyboard.press("Enter");
      await expect.poll(async () => (await item(page, id)).item.start).toBe(1.5 * FLICKS);

      // Opacity: drag the slider's thumb to the middle of its rail, then one step down by key.
      const rail = f.panel.locator(".cw-slider__track");
      const box = await rail.boundingBox();
      const thumb = await f.opacity.boundingBox();
      if (box === null || thumb === null) throw new Error("no opacity slider");
      await page.mouse.move(thumb.x + thumb.width / 2, thumb.y + thumb.height / 2);
      await page.mouse.down();
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2, { steps: 10 });
      await page.mouse.up();
      await expect.poll(async () => (await item(page, id)).visual.opacity).toBeLessThan(0.65);
      const dragged = Math.round((await item(page, id)).visual.opacity * 100);
      expect(dragged).toBeGreaterThan(35);
      await expect(f.opacity).toHaveAttribute("aria-valuetext", `${dragged} %`);
      await f.opacity.press("ArrowLeft");
      await expect
        .poll(async () => Math.round((await item(page, id)).visual.opacity * 100))
        .toBe(dragged - 1);

      // Position X: double-click to type a value.
      await f.positionX.dblclick();
      await page.keyboard.type("120");
      await page.keyboard.press("Enter");
      await expect.poll(async () => (await item(page, id)).visual.transform.position.x).toBe(120);
      await expect(f.positionX).toHaveAttribute("aria-valuenow", "120");

      // Position Y: scrub by dragging the field to the right.
      const y = await f.positionY.boundingBox();
      if (y === null) throw new Error("no position Y field");
      await page.mouse.move(y.x + y.width / 2, y.y + y.height / 2);
      await page.mouse.down();
      await page.mouse.move(y.x + y.width / 2 + 60, y.y + y.height / 2, { steps: 15 });
      await page.mouse.up();
      await expect
        .poll(async () => (await item(page, id)).visual.transform.position.y)
        .toBeGreaterThan(0);
      const scrubbed = (await item(page, id)).visual.transform.position.y;
      await expect(f.positionY).toHaveAttribute("aria-valuenow", String(scrubbed));

      // Scale typed; rotation by keys (Up = 1°, Shift+Up = 10°); gain typed in dB.
      await f.scale.dblclick();
      await page.keyboard.type("150");
      await page.keyboard.press("Enter");
      await expect.poll(async () => (await item(page, id)).visual.transform.scale.x).toBe(1.5);
      await expect(f.scale).toHaveAttribute("aria-valuetext", "150 %");
      await f.rotation.focus();
      await page.keyboard.press("ArrowUp");
      await page.keyboard.press("Shift+ArrowUp");
      await expect.poll(async () => (await item(page, id)).visual.transform.rotation).toBe(11);
      await f.gain.dblclick();
      await page.keyboard.type("-6");
      await page.keyboard.press("Enter");
      await expect.poll(async () => (await item(page, id)).audio.gain).toBe(-6);
      await expect(f.gain).toHaveAttribute("aria-valuetext", "-6.0 dB");
      await snapshot(page, info, "inspector edited");
    } finally {
      await close();
      work.remove();
    }
  });
});
