// SPDX-License-Identifier: GPL-3.0-or-later
// F5 Edit on the timeline (docs/user-flows.md): select, move by dragging, trim by an edge, split
// at the playhead (key and button), delete, zoom in and out. Linked audio follows its video.
// Ctrl+wheel zoom eases anchored at the pointer, and an agent's marker and in/out show on the
// ruler (WP-2A.09). The razor, the other tools and the zoom buttons are F61 and the ripple,
// roll, slip and slide tools F38 (WP-2A.10).
import type { Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import {
  clipAt,
  createProject,
  drag,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  importWithButton,
  inspector,
  laneY,
  launchFlowApp,
  layout,
  rulerY,
  sec,
  sequence,
  shortcut,
  snapshot,
  timecode,
  timelineCanvas,
  timelineGeometry,
  timelineTools,
  toolbar,
  workspace,
  xAt,
} from "./helpers.ts";

/**
 * How many pixels of `region` (page CSS px) the person sees in colour `rgb` (within a small
 * tolerance for anti-aliasing): a screenshot of the window, decoded and counted.
 */
async function colourCount(
  page: Page,
  region: { x: number; y: number; width: number; height: number },
  rgb: readonly [number, number, number],
): Promise<number> {
  const png = await page.screenshot({
    clip: {
      x: Math.round(region.x),
      y: Math.round(region.y),
      width: Math.max(1, Math.round(region.width)),
      height: Math.max(1, Math.round(region.height)),
    },
    animations: "allow",
    caret: "initial",
  });
  // Decoded in the page (the test has no PNG decoder); only reads the screenshot's bytes.
  return page.evaluate(
    async ({ base64, rgb }) => {
      const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
      const bitmap = await createImageBitmap(new Blob([bytes], { type: "image/png" }));
      const canvas = new OffscreenCanvas(bitmap.width, bitmap.height);
      const ctx = canvas.getContext("2d");
      if (ctx === null) throw new Error("no 2D context");
      ctx.drawImage(bitmap, 0, 0);
      const { data } = ctx.getImageData(0, 0, bitmap.width, bitmap.height);
      let n = 0;
      for (let i = 0; i < data.length; i += 4) {
        const d =
          Math.abs((data[i] ?? 0) - rgb[0]) +
          Math.abs((data[i + 1] ?? 0) - rgb[1]) +
          Math.abs((data[i + 2] ?? 0) - rgb[2]);
        if (d < 60) n++;
      }
      return n;
    },
    { base64: png.toString("base64"), rgb },
  );
}

test.describe("F5 edit on the timeline", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("move, trim both edges, split at the playhead, delete and zoom with the pointer and keys", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Edits", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect
        .poll(() => layout(page))
        .toEqual([
          { track: "V1", clips: [[1, 10]] },
          { track: "A1", clips: [[1, 10]] },
        ]);
      await editorCaughtUp(page);
      const fields = inspector(page);
      const undo = toolbar(page).getByRole("button", { name: /^Undo/ });

      // Zoom out (- twice on the focused timeline) so the whole clip and room after it show.
      const zoom = async () =>
        (await editorCall<{ zoom: number | null }>(page, "view.get")).zoom ?? 100;
      await timelineCanvas(page).locator("canvas").focus();
      await page.keyboard.press("-");
      await expect.poll(zoom).toBe(80);
      await page.keyboard.press("-");
      await expect.poll(zoom).toBeCloseTo(64, 6);

      // Move: drag the clip's body 2 s to the right. Its audio follows.
      let g = await timelineGeometry(page);
      const px = (seconds: number) => seconds * g.pxPerSec;
      let clip = clipAt(g, "V1", 1);
      await drag(
        page,
        { x: xAt(g, clip.start) + px(1), y: laneY(g, "V1") },
        { x: xAt(g, clip.start) + px(3), y: laneY(g, "V1") },
      );
      await expect
        .poll(() => layout(page))
        .toEqual([
          { track: "V1", clips: [[3, 10]] },
          { track: "A1", clips: [[3, 10]] },
        ]);
      await expect(undo).toHaveAccessibleName("Undo Move clip");
      await expect(fields.start).toHaveValue("00:00:03:00");

      // Trim the end: drag the right edge 4 s to the left.
      g = await timelineGeometry(page);
      clip = clipAt(g, "V1", 3);
      const end = xAt(g, clip.start + clip.duration);
      await drag(
        page,
        { x: end - 2, y: laneY(g, "V1") },
        { x: end - 2 - px(4), y: laneY(g, "V1") },
      );
      await expect
        .poll(() => layout(page))
        .toEqual([
          { track: "V1", clips: [[3, 6]] },
          { track: "A1", clips: [[3, 6]] },
        ]);
      await expect(undo).toHaveAccessibleName("Undo Trim clip");
      await expect(fields.duration).toHaveValue("00:00:06:00");

      // Trim the start: drag the left edge 1 s to the right.
      g = await timelineGeometry(page);
      const start = xAt(g, sec(3));
      await drag(
        page,
        { x: start + 2, y: laneY(g, "V1") },
        { x: start + 2 + px(1), y: laneY(g, "V1") },
      );
      await expect
        .poll(() => layout(page))
        .toEqual([
          { track: "V1", clips: [[4, 5]] },
          { track: "A1", clips: [[4, 5]] },
        ]);
      await expect(fields.start).toHaveValue("00:00:04:00");
      await expect(fields.duration).toHaveValue("00:00:05:00");

      // Split at the playhead: click the ruler at 6 s, then S. The selected clip splits in two.
      g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(6)), rulerY(g));
      await expect(timecode(page)).toHaveText("00:00:06:00");
      await page.keyboard.press("s");
      await expect
        .poll(() => layout(page))
        .toEqual([
          {
            track: "V1",
            clips: [
              [4, 2],
              [6, 3],
            ],
          },
          {
            track: "A1",
            clips: [
              [4, 2],
              [6, 3],
            ],
          },
        ]);
      await editorCaughtUp(page);

      // Split again with the toolbar's button at 7.5 s (nothing selected: the clip under it).
      g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(0.5)), laneY(g, "V1")); // empty lane: clears selection
      await expect(page.getByText("Nothing selected")).toBeVisible();
      await page.mouse.click(xAt(g, sec(7.5)), rulerY(g));
      await expect(timecode(page)).toHaveText("00:00:07:15");
      await timelineTools(page).getByRole("button", { name: "Split at playhead" }).click();
      await expect
        .poll(() => layout(page))
        .toEqual([
          {
            track: "V1",
            clips: [
              [4, 2],
              [6, 1.5],
              [7.5, 1.5],
            ],
          },
          {
            track: "A1",
            clips: [
              [4, 2],
              [6, 1.5],
              [7.5, 1.5],
            ],
          },
        ]);
      await editorCaughtUp(page);
      await snapshot(page, info, "split in three");

      // Delete: click the middle clip and press Delete. Its audio goes with it; no ripple.
      g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(6.75)), laneY(g, "V1"));
      await expect(fields.start).toHaveValue("00:00:06:00");
      await page.keyboard.press("Delete");
      await expect
        .poll(() => layout(page))
        .toEqual([
          {
            track: "V1",
            clips: [
              [4, 2],
              [7.5, 1.5],
            ],
          },
          {
            track: "A1",
            clips: [
              [4, 2],
              [7.5, 1.5],
            ],
          },
        ]);
      await editorCaughtUp(page);
      // Backspace deletes too.
      g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(8)), laneY(g, "A1"));
      await page.keyboard.press("Backspace");
      await expect
        .poll(() => layout(page))
        .toEqual([
          { track: "V1", clips: [[4, 2]] },
          { track: "A1", clips: [[4, 2]] },
        ]);
      await editorCaughtUp(page);

      // Zoom in: + on the timeline, then Ctrl+wheel over it.
      await timelineCanvas(page).locator("canvas").focus();
      await page.keyboard.press("+");
      await expect.poll(zoom).toBeCloseTo(80, 6);
      g = await timelineGeometry(page);
      const pointerX = xAt(g, sec(5));
      await page.mouse.move(pointerX, laneY(g, "V1"));
      await page.keyboard.down("Control");
      await page.mouse.wheel(0, -200);
      await page.keyboard.up("Control");
      // A wheel notch eases to its zoom (e^0.4 more) over a few frames, anchored at the pointer:
      // the moment under it stays under it.
      await expect.poll(zoom).toBeCloseTo(80 * Math.exp(0.4), 6);
      await expect
        .poll(async () => Math.abs(xAt(await timelineGeometry(page), sec(5)) - pointerX))
        .toBeLessThan(1.5);
      await snapshot(page, info, "zoomed in");

      // An agent marks the cut: its marker (in the color it chose) and the in/out show on the
      // ruler (WP-2A.09). Placed away from the playhead, whose line and flag are marigold too.
      g = await timelineGeometry(page);
      const { playhead } = await editorCall<{ playhead: number }>(page, "view.get");
      const firstSec = g.scrollX / sec(1) + 0.5;
      const spots = [1, 2, 3, 4, 5, 6].map((n) => firstSec + n);
      const free = spots.filter((t) => Math.abs(sec(t) - playhead) > sec(1));
      const [markAt, inAt] = free;
      if (markAt === undefined || inAt === undefined) throw new Error("no room on the ruler");
      const seqId = (await sequence(page)).id;
      await editorCall(page, "timeline.add_marker", {
        sequence: seqId,
        t: sec(markAt),
        label: "Cut here",
        color: "#00e5ff",
      });
      await editorCall(page, "timeline.set_in_out", {
        sequence: seqId,
        start: sec(inAt),
        end: sec(inAt + 0.5),
      });
      await editorCaughtUp(page);
      const rulerBottom = g.box.y + g.rulerHeight;
      await expect
        .poll(() =>
          colourCount(
            page,
            { x: xAt(g, sec(markAt)) - 5, y: rulerBottom - 14, width: 10, height: 12 },
            [0x00, 0xe5, 0xff],
          ),
        )
        .toBeGreaterThan(10);
      // The in point's marigold edge.
      await expect
        .poll(() =>
          colourCount(
            page,
            { x: xAt(g, sec(inAt)) - 2, y: g.box.y + 4, width: 5, height: g.rulerHeight - 8 },
            [0xf2, 0xb3, 0x3d],
          ),
        )
        .toBeGreaterThan(3);
      await snapshot(page, info, "marker and in-out");

      // The P1 toolbar shows the later tools disabled.
      for (const tool of ["Razor", "Ripple", "Zoom in", "Zoom out"]) {
        await expect(timelineTools(page).getByRole("button", { name: tool })).toBeDisabled();
      }
    } finally {
      await close();
      work.remove();
    }
  });

  test("rename a track: double-click its name or F2, Enter commits, Esc keeps, one Ctrl+Z undoes", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Rename", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect
        .poll(() => layout(page))
        .toEqual([
          { track: "V1", clips: [[1, 10]] },
          { track: "A1", clips: [[1, 10]] },
        ]);
      await editorCaughtUp(page);
      const names = async () => (await sequence(page)).tracks.map((t) => t.name);
      const field = page.getByRole("textbox", { name: "Track name" });
      const undo = toolbar(page).getByRole("button", { name: /^Undo/ });

      // A point inside a track's name in its header (right of the kind icon, left of the toggles).
      const onName = async (trackName: string) => {
        const g = await timelineGeometry(page);
        const tokens = await page.evaluate(() => {
          const cs = getComputedStyle(document.documentElement);
          const px = (name: string) => Number.parseFloat(cs.getPropertyValue(name));
          return { space2: px("--space-2"), iconSm: px("--icon-sm") };
        });
        const nameX = tokens.space2 + tokens.iconSm + tokens.space2;
        return { x: g.box.x + nameX + 6, y: laneY(g, trackName) };
      };

      // Double-click V1's name, type B-roll, Enter: the header shows it.
      let at = await onName("V1");
      await page.mouse.dblclick(at.x, at.y);
      await expect(field).toBeFocused();
      await expect(field).toHaveValue("V1");
      await page.keyboard.type("B-roll");
      await snapshot(page, info, "renaming V1");
      await page.keyboard.press("Enter");
      await expect(field).toBeHidden();
      await expect.poll(names).toEqual(["B-roll", "A1"]);
      await editorCaughtUp(page);
      await expect(undo).toHaveAccessibleName("Undo Change track");
      await snapshot(page, info, "V1 renamed to B-roll");

      // One Ctrl+Z restores V1 (the rename was one undo step).
      await shortcut(app, "Control+Z");
      await expect.poll(names).toEqual(["V1", "A1"]);
      await editorCaughtUp(page);

      // Esc in the middle of typing keeps the old name; no timeline key fired while typing.
      at = await onName("V1");
      await page.mouse.dblclick(at.x, at.y);
      await expect(field).toBeFocused();
      await page.keyboard.type("Draft s");
      await expect(field).toHaveValue("Draft s");
      await page.keyboard.press("Escape");
      await expect(field).toBeHidden();
      expect(await names()).toEqual(["V1", "A1"]);
      expect(await layout(page)).toEqual([
        { track: "V1", clips: [[1, 10]] },
        { track: "A1", clips: [[1, 10]] },
      ]);

      // A blank name is refused: the input stays, says why, and Esc leaves the old name.
      at = await onName("V1");
      await page.mouse.dblclick(at.x, at.y);
      await expect(field).toBeFocused();
      await page.keyboard.press("Control+A");
      await page.keyboard.type("   ");
      await page.keyboard.press("Enter");
      await expect(
        page.getByRole("alert").filter({ hasText: "A track needs a name" }),
      ).toBeVisible();
      await expect(field).toHaveValue("V1");
      await page.keyboard.press("Escape");
      await expect(field).toBeHidden();
      expect(await names()).toEqual(["V1", "A1"]);

      // F2 on a selected track opens the same input: click A1's header, F2, rename, Enter.
      at = await onName("A1");
      await page.mouse.click(at.x, at.y);
      await page.keyboard.press("F2");
      await expect(field).toBeFocused();
      await expect(field).toHaveValue("A1");
      await page.keyboard.press("Control+A");
      await page.keyboard.type("  Music ");
      await page.keyboard.press("Enter");
      await expect(field).toBeHidden();
      await expect.poll(names).toEqual(["V1", "Music"]);
    } finally {
      await close();
      work.remove();
    }
  });
});
