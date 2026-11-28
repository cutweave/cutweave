// SPDX-License-Identifier: GPL-3.0-or-later
// F11 Engine crash mid-edit (docs/user-flows.md): edit, the engine dies, keep editing. The chip
// says "Reconnecting" and comes back, every edit is applied exactly once and nothing is lost.
// The crash is fault injection (`killEngine`, a SIGKILL of the engine process, WP-1L.03); the
// edits are the person's: drops, drags and a split typed while the engine is gone.
// e2e/resync.spec.ts covers the protocol side (resend, dedup, port change) call by call.
import { readFileSync } from "node:fs";
import { engineStatus, expect, killEngine, test } from "../fixtures.ts";
import {
  clickClip,
  clipAt,
  createProject,
  dirtyDot,
  drag,
  dropCard,
  editorCaughtUp,
  engineChip,
  freeMainTrack,
  hasRealEngine,
  importWithButton,
  laneY,
  launchFlowApp,
  layout,
  rulerY,
  sec,
  shortcut,
  snapshot,
  timecode,
  timelineGeometry,
  toast,
  workspace,
  xAt,
} from "./helpers.ts";

test.describe("F11 engine crash mid-edit", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("the editor reconnects; edits before, during and after the crash each land once", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await freeMainTrack(app, page); // clips stay where they are dropped
      const doc = await createProject(app, page, "Crash", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect
        .poll(() => layout(page))
        .toEqual([
          { track: "V1", clips: [[1, 10]] },
          { track: "A1", clips: [[1, 10]] },
        ]);
      await editorCaughtUp(page);
      // Move it to 1.5 s, select it and put the playhead at 6 s.
      let g = await timelineGeometry(page);
      const id = clipAt(g, "V1", 1).id;
      await drag(
        page,
        { x: xAt(g, sec(2)), y: laneY(g, "V1") },
        { x: xAt(g, sec(2.5)), y: laneY(g, "V1") },
      );
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1.5, 10]]);
      await clickClip(page, id);
      g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(6)), rulerY(g));
      await expect(timecode(page)).toHaveText("00:00:06:00");
      await expect(dirtyDot(page)).toBeVisible();

      // The engine dies.
      await killEngine(app);
      await expect(engineChip(page)).toHaveText("Reconnecting");
      // S while it is gone: the split waits for the engine and goes out once it is back.
      await page.keyboard.press("s");
      await expect(toast(page, "Engine restarted — recovered")).toContainText("Crash.cutweave");
      await expect(engineChip(page)).toHaveText("Engine connected");
      await expect.poll(async () => (await engineStatus(app))?.generation).toBe(2);

      // Everything is there once: the drop, the move and the split.
      await expect
        .poll(() => layout(page))
        .toEqual([
          {
            track: "V1",
            clips: [
              [1.5, 4.5],
              [6, 5.5],
            ],
          },
          {
            track: "A1",
            clips: [
              [1.5, 4.5],
              [6, 5.5],
            ],
          },
        ]);
      await editorCaughtUp(page);
      await snapshot(page, info, "after the crash");

      // A drag cut short by a crash: the clip goes back and the notice says the engine restarted
      // (#148), not that the project changed.
      g = await timelineGeometry(page);
      const grab = { x: xAt(g, sec(3)), y: laneY(g, "V1") };
      await page.mouse.move(grab.x, grab.y);
      await page.mouse.down();
      await page.mouse.move(grab.x - 12, grab.y, { steps: 3 });
      await page.mouse.move(grab.x - 40, grab.y, { steps: 3 });
      await killEngine(app);
      await expect(engineChip(page)).toHaveText("Reconnecting");
      await page.mouse.up();
      await expect(toast(page, "Clip not moved")).toContainText(
        "The engine restarted, so the edit was undone. Try again.",
      );
      await expect(engineChip(page)).toHaveText("Engine connected");
      await expect.poll(async () => (await engineStatus(app))?.generation).toBe(3);
      await expect
        .poll(async () => (await layout(page))[0]?.clips)
        .toEqual([
          [1.5, 4.5],
          [6, 5.5],
        ]);
      await editorCaughtUp(page);

      // Keep editing: drag the left half 1 s earlier (clear of the toasts), then save.
      g = await timelineGeometry(page);
      await drag(
        page,
        { x: xAt(g, sec(3)), y: laneY(g, "V1") },
        { x: xAt(g, sec(2)), y: laneY(g, "V1") },
      );
      await expect
        .poll(async () => (await layout(page))[0]?.clips)
        .toEqual([
          [0.5, 4.5],
          [6, 5.5],
        ]);
      await shortcut(app, "Control+S");
      await expect(dirtyDot(page)).toBeHidden();
      const saved = JSON.parse(readFileSync(doc, "utf8")) as {
        sequences: Record<string, { tracks: Record<string, { items: Record<string, unknown> }> }>;
      };
      const onDisk = Object.values(saved.sequences).flatMap((s) =>
        Object.values(s.tracks).flatMap((t) => Object.keys(t.items)),
      );
      expect(onDisk).toHaveLength(4);
    } finally {
      await close();
      work.remove();
    }
  });
});
