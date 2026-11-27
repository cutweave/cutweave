// SPDX-License-Identifier: GPL-3.0-or-later
// F39 Overlays, linked and attached clips (docs/user-flows.md, WP-2A.05, WP-2A.10). On a magnetic
// V1 with two clips and an overlay track added from the "Add track" button: the clip menu's
// Unlink clips, then the picture dragged alone leaves its sound where it was; a still dropped
// onto the busy overlay track where it does not fit gets a new track above (Ctrl+Z); a title
// attached to a main-track clip (clip menu) follows that clip when it is dragged after the other
// one, and stays where it was, detached, when the clip is deleted; a dragged clip lands exactly
// on the playhead, and on a marker added with the ruler menu's "Add marker here". Real clicks,
// drags and keys; only the OS file picker is stubbed.
import type { Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import {
  clipAt,
  clipBox,
  createProject,
  drag,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  importWithButton,
  laneY,
  launchFlowApp,
  layout,
  movePlayhead,
  rulerY,
  sec,
  sequence,
  shortcut,
  showTimeline,
  snapshot,
  type TimelineGeometry,
  timelineGeometry,
  workspace,
  xAt,
} from "./helpers.ts";

/** A track's clips as `[start, duration]` seconds. */
async function on(page: Page, track: string): Promise<[number, number][]> {
  return (await layout(page)).find((t) => t.track === track)?.clips ?? [];
}

/** What the engine says a clip is attached to. */
async function attachedTo(page: Page, id: string): Promise<string | null> {
  return (
    (await editorCall<{ attached_to?: string | null }>(page, "item.get", { item: id }))
      .attached_to ?? null
  );
}

test.describe("F39 overlays, linked and attached clips", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("unlink and move the picture alone, a busy overlay drop, attach and detach, snap to the playhead and a marker", async () => {
    test.setTimeout(300_000);
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Overlays", work.dir);
      await importWithButton(app, page, [
        work.file("interview.mp4"),
        work.file("broll-city.mp4"),
        work.file("broll-coffee.mp4"),
        work.file("logo.png"),
      ]);
      const menu = page.getByRole("menu");

      // V1 becomes the magnetic main track (its header menu); two clips butt from 0.
      let g = await timelineGeometry(page);
      await page.mouse.click(g.box.x + 12, laneY(g, "V1"), { button: "right" });
      await page
        .getByRole("menu", { name: "Track actions for V1" })
        .getByRole("menuitemcheckbox", { name: /^Magnetic main track/ })
        .click();
      await expect(menu).toHaveCount(0);
      await expect
        .poll(async () => {
          const seq = await editorCall<{ tracks: { name: string; magnetic?: boolean }[] }>(
            page,
            "timeline.get",
            { sequence: (await sequence(page)).id, depth: 1 },
          );
          return seq.tracks.find((t) => t.name === "V1")?.magnetic;
        })
        .toBe(true);
      // Zoom out (the toolbar) until everything the flow places will show.
      await showTimeline(page, 37);
      await dropCard(page, "interview.mp4", "V1", 0);
      await expect.poll(() => on(page, "V1")).toEqual([[0, 10]]);
      await editorCaughtUp(page);
      await dropCard(page, "broll-city.mp4", "V1", 11);
      await expect
        .poll(() => on(page, "V1"))
        .toEqual([
          [0, 10],
          [10, 10],
        ]);
      await editorCaughtUp(page);

      // An overlay track from the "Add track" button above the headers.
      await page.getByRole("button", { name: "Add track", exact: true }).click();
      await page.getByRole("menuitem", { name: "Video track" }).click();
      await expect.poll(async () => (await sequence(page)).tracks.length).toBe(3);
      await editorCaughtUp(page);
      const overlay = (await sequence(page)).tracks.find(
        (t) => t.kind === "video" && t.name !== "V1",
      )?.name;
      if (overlay === undefined) throw new Error("no overlay track");

      // The coffee B-roll on the overlay at 32 s (its sound goes to A1, after the others).
      await movePlayhead(page, 32);
      await dropCard(page, "broll-coffee.mp4", overlay, 32);
      await expect.poll(() => on(page, overlay)).toEqual([[32, 10]]);
      await editorCaughtUp(page);

      // Its clip menu: Unlink clips. Then the picture dragged 2 s right moves alone.
      g = await timelineGeometry(page);
      const coffee = clipAt(g, overlay, 32).id;
      let box = clipBox(g, coffee);
      await page.mouse.click(box.x + 20, box.y, { button: "right" });
      await page
        .getByRole("menu", { name: /^Clip actions for/ })
        .getByRole("menuitem", { name: /^Unlink clips/ })
        .click();
      await expect(menu).toHaveCount(0);
      await expect
        .poll(
          async () =>
            (await sequence(page)).tracks.find((t) => t.name === overlay)?.items[0]?.linked_group,
        )
        .toBeNull();
      await editorCaughtUp(page);
      await page.mouse.click(box.x + 20, box.y);
      await drag(page, { x: box.x + 20, y: box.y }, { x: box.x + 20 + 2 * g.pxPerSec, y: box.y });
      await expect.poll(() => on(page, overlay)).toEqual([[34, 10]]);
      expect(await on(page, "A1")).toEqual([
        [0, 10],
        [10, 10],
        [32, 10],
      ]);
      await editorCaughtUp(page);

      // The playhead at 13 s; the coffee dragged so its start comes 3 px past it lands on it.
      g = await timelineGeometry(page);
      const near = (geometry: TimelineGeometry, t: number) => xAt(geometry, sec(t)) + 3;
      await movePlayhead(page, 13);
      box = clipBox(g, coffee);
      let grab = { x: box.x + 20, y: box.y };
      await drag(page, grab, { x: near(g, 13) + 20, y: box.y });
      await expect.poll(() => on(page, overlay)).toEqual([[13, 10]]);
      await editorCaughtUp(page);

      // The ruler menu's "Add marker here" at 34 s; the coffee dragged near it lands on it.
      g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(34)) + 0.5, rulerY(g), { button: "right" });
      await page
        .getByRole("menu", { name: "Ruler actions" })
        .getByRole("menuitem", { name: /^Add marker here/ })
        .click();
      await expect
        .poll(async () =>
          (
            await editorCall<{ markers: { t: number }[] }>(page, "timeline.list_markers", {
              sequence: (await sequence(page)).id,
            })
          ).markers.map((m) => m.t),
        )
        .toEqual([sec(34)]);
      await editorCaughtUp(page);
      box = clipBox(g, coffee);
      grab = { x: box.x + 20, y: box.y };
      await drag(page, grab, { x: near(g, 34) + 20, y: box.y });
      await expect.poll(() => on(page, overlay)).toEqual([[34, 10]]);
      await editorCaughtUp(page);

      // The logo dropped onto the overlay at 31 s does not fit before the coffee: a new track
      // above takes it. Ctrl+Z.
      const tracksBefore = (await sequence(page)).tracks.length;
      await movePlayhead(page, 31);
      await dropCard(page, "logo.png", overlay, 31);
      await expect.poll(async () => (await sequence(page)).tracks.length).toBe(tracksBefore + 1);
      const seq = await sequence(page);
      const above = seq.tracks.findIndex((t) => t.items.some((i) => i.start === sec(31)));
      expect(above).toBeLessThan(seq.tracks.findIndex((t) => t.name === overlay));
      expect(await on(page, overlay)).toEqual([[34, 10]]);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await sequence(page)).tracks.length).toBe(tracksBefore);
      await editorCaughtUp(page);

      // A title (the logo) on the overlay at 2 s, attached to the interview from its menu; the
      // interview dragged after the B-roll takes it along.
      await movePlayhead(page, 2);
      await dropCard(page, "logo.png", overlay, 2);
      await expect
        .poll(() => on(page, overlay))
        .toEqual([
          [2, 5],
          [34, 10],
        ]);
      await editorCaughtUp(page);
      g = await timelineGeometry(page);
      const title = clipAt(g, overlay, 2).id;
      const interview = clipAt(g, "V1", 0).id;
      box = clipBox(g, title);
      await page.mouse.click(box.x + 10, box.y, { button: "right" });
      await page
        .getByRole("menu", { name: /^Clip actions for/ })
        .getByRole("menuitem", { name: /^Attach to main clip/ })
        .click();
      await expect.poll(() => attachedTo(page, title)).toBe(interview);
      await editorCaughtUp(page);
      box = clipBox(g, interview);
      await drag(page, { x: box.x + 20, y: box.y }, { x: box.x + 20 + 15 * g.pxPerSec, y: box.y });
      await expect
        .poll(() => on(page, "V1"))
        .toEqual([
          [0, 10],
          [10, 10],
        ]);
      await expect
        .poll(async () => (await sequence(page)).tracks.find((t) => t.name === "V1")?.items[1]?.id)
        .toBe(interview);
      await expect
        .poll(() => on(page, overlay))
        .toEqual([
          [12, 5],
          [34, 10],
        ]);
      await editorCaughtUp(page);
      await snapshot(page, info, "title follows its clip");

      // Delete the interview: the title stays where it was, no longer attached.
      g = await timelineGeometry(page);
      box = clipBox(g, interview);
      await page.mouse.click(box.x + 20, box.y);
      await page.keyboard.press("Delete");
      await expect.poll(() => on(page, "V1")).toEqual([[0, 10]]);
      expect(await on(page, overlay)).toEqual([
        [12, 5],
        [34, 10],
      ]);
      expect(await attachedTo(page, title)).toBeNull();
      await editorCaughtUp(page);
    } finally {
      await close();
      work.remove();
    }
  });
});
