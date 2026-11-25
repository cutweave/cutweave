// SPDX-License-Identifier: GPL-3.0-or-later
// F6 Inspector (docs/user-flows.md): select a clip; edit its name, start and duration (typed
// timecodes), opacity, position, scale, rotation and gain, by typing, dragging and keys. The
// timeline follows timing changes, and input that is not a time is refused with a message.
// Second test (WP-2H.03a): two clips edited at once (one undo step), typed expressions, keyframes
// from the diamond (add, next, previous, remove, reset) and the Track section.
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
  laneY,
  launchFlowApp,
  layout,
  positionOf,
  rulerY,
  sec,
  sequence,
  shortcut,
  snapshot,
  timecode,
  timelineGeometry,
  workspace,
  xAt,
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
      // The inspector may still be re-laying out after the edits above (the error note closing):
      // drag only once the rail stays where it is, as a person would aim at what they see.
      let box = await rail.boundingBox();
      await expect
        .poll(async () => {
          const next = await rail.boundingBox();
          const steady =
            next !== null &&
            box !== null &&
            Math.abs(next.x - box.x) < 0.5 &&
            Math.abs(next.y - box.y) < 0.5 &&
            Math.abs(next.width - box.width) < 0.5;
          box = next;
          return steady;
        })
        .toBe(true);
      const thumb = await f.opacity.boundingBox();
      if (box === null || thumb === null) throw new Error("no opacity slider");
      await page.mouse.move(thumb.x + thumb.width / 2, thumb.y + thumb.height / 2);
      await page.mouse.down();
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2, { steps: 10 });
      await page.mouse.up();
      await expect.poll(async () => (await item(page, id)).visual.opacity).toBeLessThan(0.65);
      const dragged = Math.round((await item(page, id)).visual.opacity * 100);
      expect(dragged).toBeGreaterThan(35);
      await expect(f.opacity).toHaveAttribute("aria-valuetext", `${dragged}%`);
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
      // Aim at what is on screen: a reset button appears beside an edited row and moves the field.
      let y = await f.positionY.boundingBox();
      await expect
        .poll(async () => {
          const next = await f.positionY.boundingBox();
          const steady = next !== null && y !== null && Math.abs(next.x - y.x) < 0.5;
          y = next;
          return steady;
        })
        .toBe(true);
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

  test("two clips edit as one, expressions work, and the diamond keys a property", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Inspector video", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
      await editorCaughtUp(page);
      const seq = await sequence(page);
      const videoId = seq.tracks.find((t) => t.name === "V1")?.items[0]?.id;
      const audioId = seq.tracks.find((t) => t.name === "A1")?.items[0]?.id;
      if (videoId === undefined || audioId === undefined) throw new Error("expected video + audio");
      await clickClip(page, videoId);
      const f = inspector(page);

      // The sections the schema defines, and the clip's source.
      for (const name of ["Transform", "Crop", "Flip", "Compositing", "Track"]) {
        await expect(f.panel.getByRole("button", { name, exact: true })).toBeVisible();
      }
      await expect(f.panel.getByTestId("clip-source")).toHaveText("interview.mp4");

      // Select the picture and its sound: opacity for both with one drag, one undo step.
      let g = await timelineGeometry(page);
      await page.keyboard.down("Shift");
      await page.mouse.click(xAt(g, sec(3)), laneY(g, "A1"));
      await page.keyboard.up("Shift");
      await expect(f.panel.getByText(/2 clips selected/)).toBeVisible();
      await expect(f.panel.getByText(/sit on different tracks/)).toBeVisible();
      const rail = f.panel.locator(".cw-slider__track");
      let box = await rail.boundingBox();
      await expect
        .poll(async () => {
          const next = await rail.boundingBox();
          const steady =
            next !== null &&
            box !== null &&
            Math.abs(next.x - box.x) < 0.5 &&
            Math.abs(next.y - box.y) < 0.5;
          box = next;
          return steady;
        })
        .toBe(true);
      const thumb = await f.opacity.boundingBox();
      if (box === null || thumb === null) throw new Error("no opacity slider");
      await page.mouse.move(thumb.x + thumb.width / 2, thumb.y + thumb.height / 2);
      await page.mouse.down();
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2, { steps: 10 });
      await page.mouse.up();
      await expect.poll(async () => (await item(page, videoId)).visual.opacity).toBeLessThan(0.65);
      const half = (await item(page, videoId)).visual.opacity;
      expect(half).toBeGreaterThan(0.35);
      await expect.poll(async () => (await item(page, audioId)).visual.opacity).toBe(half);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await item(page, videoId)).visual.opacity).toBe(1);
      await expect.poll(async () => (await item(page, audioId)).visual.opacity).toBe(1);

      // A typed expression on both: rotation +15 adds to the value.
      await f.rotation.dblclick();
      await page.keyboard.type("+15");
      await page.keyboard.press("Enter");
      await expect.poll(async () => (await item(page, videoId)).visual.transform.rotation).toBe(15);
      await expect.poll(async () => (await item(page, audioId)).visual.transform.rotation).toBe(15);
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await item(page, audioId)).visual.transform.rotation).toBe(0);

      // One clip: scale *2 keeps the aspect, 50% takes half of the shown value.
      await clickClip(page, videoId);
      await f.scale.dblclick();
      await page.keyboard.type("*2");
      await page.keyboard.press("Enter");
      await expect.poll(async () => (await item(page, videoId)).visual.transform.scale.x).toBe(2);
      await expect(f.scale).toHaveAttribute("aria-valuenow", "200");
      await f.scale.dblclick();
      await page.keyboard.type("50%");
      await page.keyboard.press("Enter");
      await expect.poll(async () => (await item(page, videoId)).visual.transform.scale.x).toBe(1);
      await editorCaughtUp(page);

      // Keyframes: the playhead at 2 s (1 s into the clip), add a key on Position with the diamond.
      g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(2)), rulerY(g));
      await expect(timecode(page)).toHaveText("00:00:02:00");
      const keys = f.panel.getByRole("group", { name: "Keyframes for Position" });
      await keys.getByRole("button", { name: "Add keyframe for Position" }).click();
      await expect.poll(async () => (await positionOf(page, videoId)).keyframes?.length).toBe(1);
      // At 4 s, typing a position keys it there (the property is animated now).
      g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(4)), rulerY(g));
      await expect(timecode(page)).toHaveText("00:00:04:00");
      await f.positionX.dblclick();
      await page.keyboard.type("100");
      await page.keyboard.press("Enter");
      await expect.poll(async () => (await positionOf(page, videoId)).keyframes?.length).toBe(2);
      const position = await positionOf(page, videoId);
      expect(position.keyframes?.map((k) => [k.t / FLICKS, k.value.x])).toEqual([
        [1, 0],
        [3, 100],
      ]);
      // Halfway between the keys the shown value is the engine's interpolation.
      g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(3)), rulerY(g));
      await expect(timecode(page)).toHaveText("00:00:03:00");
      await expect(f.positionX).toHaveAttribute("aria-valuenow", "50");
      // Next and previous key move the playhead.
      await keys.getByRole("button", { name: /^Next keyframe/ }).click();
      await expect(timecode(page)).toHaveText("00:00:04:00");
      await keys.getByRole("button", { name: /^Previous keyframe/ }).click();
      await expect(timecode(page)).toHaveText("00:00:02:00");
      await snapshot(page, info, "keyframed position");
      // Remove the key under the playhead, then reset the property: static again, at its default.
      await keys.getByRole("button", { name: "Remove keyframe at playhead for Position" }).click();
      await expect.poll(async () => (await positionOf(page, videoId)).keyframes?.length).toBe(1);
      await f.panel.getByRole("button", { name: "Reset Position" }).click();
      await expect.poll(async () => (await positionOf(page, videoId)).x).toBe(0);
      await expect(keys.getByRole("button", { name: "Add keyframe for Position" })).toBeVisible();

      // The Track section: rename V1, then lock it with the icon button; the fields lock.
      const trackName = f.panel.getByTestId("track-inspector").getByLabel("Track name");
      await trackName.fill("Main video");
      await page.keyboard.press("Enter");
      await expect.poll(async () => (await sequence(page)).tracks[0]?.name).toBe("Main video");
      await f.panel.getByRole("button", { name: "Lock", exact: true }).click();
      await expect(f.panel.getByText(/track is locked/i)).toBeVisible();
      await expect(f.rotation).toHaveAttribute("aria-disabled", "true");
    } finally {
      await close();
      work.remove();
    }
  });
});
