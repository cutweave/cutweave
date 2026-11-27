// SPDX-License-Identifier: GPL-3.0-or-later
// F4 Build a timeline (docs/user-flows.md): drag clips from the bin onto the tracks, insert one
// with Enter, toggle snapping. Clips land where they are dropped (to the frame), snapping pulls
// an edge onto a nearby clip edge, and a drop onto free space too short for the clip gets a new
// track (timeline spec INV-40, WP-2A.10: a drop onto a clip overwrites it by default).
import { expect, test } from "../fixtures.ts";
import {
  clipNames,
  createProject,
  DEMO_FILES,
  dropCard,
  editorCaughtUp,
  FRAME,
  hasRealEngine,
  importWithButton,
  laneY,
  launchFlowApp,
  layout,
  mediaCard,
  sec,
  sequence,
  snapshot,
  timelineCanvas,
  timelineGeometry,
  timelineTools,
  workspace,
  xAt,
} from "./helpers.ts";

test.describe("F4 build a timeline", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("drag clips onto tracks, snap to an edge, drop without snapping, insert with Enter", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Timeline", work.dir);
      await importWithButton(
        app,
        page,
        DEMO_FILES.map((f) => work.file(f)),
      );
      const snapping = timelineTools(page).getByRole("button", { name: "Snapping" });
      await expect(snapping).toHaveAttribute("aria-pressed", "true");

      // The interview onto Video 1 at 1 s: the video and its linked audio land there.
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect
        .poll(() => layout(page))
        .toEqual([
          { track: "V1", clips: [[1, 10]] },
          { track: "A1", clips: [[1, 10]] },
        ]);
      await editorCaughtUp(page);
      await expect(page.getByText("Your timeline is empty")).toBeHidden();

      // B-roll dropped 5 px after the interview's end: snapping pulls it onto the edge (11 s).
      await dropCard(page, "broll-city.mp4", "V1", 11, 5);
      await expect.poll(() => clipNames(page, "V1")).toEqual(["interview.mp4", "broll-city.mp4"]);
      expect((await sequence(page)).tracks[0]?.items[1]?.start).toBe(sec(11));
      await editorCaughtUp(page);

      // N (with the timeline focused: a click on the empty lane before the first clip) turns
      // snapping off.
      const g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(0.3)), laneY(g, "V1"));
      await expect(timelineCanvas(page).locator("canvas")).toBeFocused();
      await page.keyboard.press("n");
      await expect(snapping).toHaveAttribute("aria-pressed", "false");

      // Music dropped 5 px after 0 s on Audio 1, now without snapping: it stays where it was
      // dropped (to the frame), and since it does not fit before the interview's sound it gets a
      // track of its own.
      await dropCard(page, "music.wav", "A1", 0, 5);
      await expect.poll(async () => (await sequence(page)).tracks.length).toBe(3);
      const music = (await sequence(page)).tracks.find(
        (t) => t.kind === "audio" && t.items.length === 1 && t.items[0]?.linked_group === null,
      )?.items[0];
      expect(music).toBeDefined();
      expect(music?.start).not.toBe(0);
      expect(Math.abs((music?.start ?? 0) - sec(0.05))).toBeLessThanOrEqual(FRAME);
      await editorCaughtUp(page);

      // The toolbar button turns snapping back on.
      await snapping.click();
      await expect(snapping).toHaveAttribute("aria-pressed", "true");

      // Enter in the bin inserts the selected card at the playhead (0 s) on Video 1, which the
      // interview occupies from 1 s: the logo (a 5 s still) gets a new video track.
      await mediaCard(page, "logo.png").click();
      await page.keyboard.press("Enter");
      await expect.poll(async () => (await sequence(page)).tracks.length).toBe(4);
      const seq = await sequence(page);
      const logoTrack = seq.tracks.find(
        (t) => t.kind === "video" && t.items.length === 1 && t.items[0]?.start === 0,
      );
      expect(logoTrack?.items[0]?.duration).toBe(sec(5));
      expect(await clipNames(page, logoTrack?.name ?? "")).toEqual(["logo.png"]);
      expect(await clipNames(page, "V1")).toEqual(["interview.mp4", "broll-city.mp4"]);
      expect(await clipNames(page, "A1")).toEqual(["interview.mp4", "broll-city.mp4"]);
      await editorCaughtUp(page);
      await snapshot(page, info, "timeline built");
    } finally {
      await close();
      work.remove();
    }
  });
});
