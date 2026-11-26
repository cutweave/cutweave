// SPDX-License-Identifier: GPL-3.0-or-later
// F48 Inspector: sound, speed and the sequence (docs/user-flows.md; WP-2H.03b). Two sound clips
// selected: volume, pan and a fade for both (one undo step each). A picture clip: speed typed as
// text, Reverse, Reset speed, Freeze frame. Nothing selected: the sequence's canvas size and
// frame rate.
import { editorCall, expect, test } from "../fixtures.ts";
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
  rulerY,
  sec,
  sequence,
  sequenceFormat,
  snapshot,
  timecode,
  timelineCanvas,
  timelineGeometry,
  workspace,
  xAt,
} from "./helpers.ts";

const BOUND = { timeout: 15_000 };

test.describe("F48 inspector: sound, speed and sequence", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("two sound clips take volume, pan and a fade; speed and the sequence format are editable", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Inspector sound", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
      await editorCaughtUp(page);
      // Zoom out (- on the focused timeline) so both clips show.
      await timelineCanvas(page).locator("canvas").focus();
      for (let i = 0; i < 5; i++) await page.keyboard.press("-");
      const zoom = async () =>
        (await editorCall<{ zoom: number | null }>(page, "view.get")).zoom ?? 100;
      await expect.poll(zoom, BOUND).toBeLessThan(40);
      await dropCard(page, "interview.mp4", "V1", 12);
      await expect
        .poll(async () => (await layout(page)).find((t) => t.track === "V1")?.clips.length)
        .toBe(2);
      await editorCaughtUp(page);
      // Undo with the toolbar button: the keys belong to a text field while it has the focus.
      const undo = () => page.getByRole("button", { name: /^Undo/ }).click();
      const seq = await sequence(page);
      const sound = seq.tracks.find((t) => t.name === "A1")?.items ?? [];
      const picture = seq.tracks.find((t) => t.name === "V1")?.items ?? [];
      const [a1, a2] = sound;
      const v1 = picture[0];
      if (a1 === undefined || a2 === undefined || v1 === undefined) {
        throw new Error("expected two clips with sound");
      }

      // The two sound clips selected: volume and pan for both.
      await clickClip(page, a1.id);
      const f = inspector(page);
      await expect(f.panel.getByRole("button", { name: "Audio", exact: true })).toBeVisible(BOUND);
      const g = await timelineGeometry(page);
      await page.keyboard.down("Shift");
      await page.mouse.click(xAt(g, sec(12)) + 60, laneY(g, "A1"));
      await page.keyboard.up("Shift");
      await expect(f.panel.getByText(/2 clips selected/)).toBeVisible(BOUND);

      const volume = f.panel.getByRole("spinbutton", { name: "Volume" });
      const pan = f.panel.getByRole("spinbutton", { name: "Pan" });
      await volume.dblclick();
      await page.keyboard.type("-6");
      await page.keyboard.press("Enter");
      await expect.poll(async () => (await item(page, a1.id)).audio.gain, BOUND).toBe(-6);
      await expect.poll(async () => (await item(page, a2.id)).audio.gain, BOUND).toBe(-6);
      await expect(volume).toHaveAttribute("aria-valuetext", "-6.0 dB");
      await pan.dblclick();
      await page.keyboard.type("-0.5");
      await page.keyboard.press("Enter");
      await expect.poll(async () => (await item(page, a1.id)).audio.pan, BOUND).toBe(-0.5);
      await expect.poll(async () => (await item(page, a2.id)).audio.pan, BOUND).toBe(-0.5);
      // One undo step takes the pan of both back; the volume stays.
      await editorCaughtUp(page);
      await undo();
      await expect.poll(async () => (await item(page, a1.id)).audio.pan, BOUND).toBe(0);
      await expect.poll(async () => (await item(page, a2.id)).audio.pan, BOUND).toBe(0);
      expect((await item(page, a1.id)).audio.gain).toBe(-6);

      // A fade in of one second on both.
      await f.panel.getByRole("spinbutton", { name: "Fade in" }).dblclick();
      await page.keyboard.type("1");
      await page.keyboard.press("Enter");
      await expect
        .poll(async () => (await item(page, a1.id)).audio.fade_in?.duration, BOUND)
        .toBe(FLICKS);
      await expect
        .poll(async () => (await item(page, a2.id)).audio.fade_in?.duration, BOUND)
        .toBe(FLICKS);
      await snapshot(page, info, "sound edited");

      // A picture clip: no Audio section; Speed typed as text halves the clip and its sound.
      await clickClip(page, v1.id);
      await expect(f.panel.getByRole("button", { name: "Audio", exact: true })).toBeHidden(BOUND);
      const speed = f.panel.getByRole("textbox", { name: "Speed" });
      await speed.fill("2x");
      await speed.press("Enter");
      await expect
        .poll(async () => (await item(page, v1.id)).source?.speed, BOUND)
        .toEqual({
          num: 2,
          den: 1,
        });
      await expect.poll(async () => (await item(page, v1.id)).item.duration, BOUND).toBe(sec(5));
      await expect.poll(async () => (await item(page, a1.id)).item.duration, BOUND).toBe(sec(5));
      await editorCaughtUp(page);
      await undo();
      await expect.poll(async () => (await item(page, v1.id)).item.duration, BOUND).toBe(sec(10));

      // Text the engine does not accept is refused with its own message; nothing changes.
      await speed.fill("500x");
      await speed.press("Enter");
      await expect(page.getByText("Change not applied")).toBeVisible(BOUND);
      expect((await item(page, v1.id)).source?.speed).toEqual({ num: 1, den: 1 });

      // Reverse, then Reset speed.
      await f.panel.getByRole("switch", { name: "Reverse" }).click();
      await expect.poll(async () => (await item(page, v1.id)).source?.reverse, BOUND).toBe(true);
      await f.panel.getByRole("button", { name: "Reset Speed", exact: true }).click();
      await expect.poll(async () => (await item(page, v1.id)).source?.reverse, BOUND).toBe(false);
      await snapshot(page, info, "speed section");

      // Freeze frame at the playhead: the clip splits and a 3 s still goes in.
      const g2 = await timelineGeometry(page);
      await page.mouse.click(xAt(g2, sec(4)), rulerY(g2));
      await expect(timecode(page)).toHaveText("00:00:04:00");
      await f.panel.getByRole("button", { name: "Freeze frame at playhead" }).click();
      await expect
        .poll(async () => (await layout(page)).find((t) => t.track === "V1")?.clips.length, BOUND)
        .toBe(4);
      await editorCaughtUp(page);

      // Nothing selected: the sequence's canvas and frame rate.
      const empty = await timelineGeometry(page);
      await page.mouse.click(xAt(empty, sec(0.4)), laneY(empty, "V1"));
      const panel = page.getByTestId("sequence-inspector");
      await expect(panel).toBeVisible(BOUND);
      await panel.getByRole("spinbutton", { name: "Width" }).dblclick();
      await page.keyboard.type("1280");
      await page.keyboard.press("Enter");
      await expect.poll(async () => (await sequenceFormat(page)).width, BOUND).toBe(1280);
      await panel.getByLabel("Frame rate").selectOption({ label: "24 fps" });
      await expect
        .poll(async () => (await sequenceFormat(page)).frame_rate, BOUND)
        .toEqual({ num: 24, den: 1 });
      await snapshot(page, info, "sequence section");
      // Both are one undo step each.
      await editorCaughtUp(page);
      await undo();
      await expect
        .poll(async () => (await sequenceFormat(page)).frame_rate, BOUND)
        .toEqual({ num: 30, den: 1 });
      expect(clipAt(await timelineGeometry(page), "V1", 1).id).toBe(v1.id);
    } finally {
      await close();
      work.remove();
    }
  });
});
