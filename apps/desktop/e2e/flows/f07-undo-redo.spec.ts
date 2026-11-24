// SPDX-License-Identifier: GPL-3.0-or-later
// F7 Undo and redo (docs/user-flows.md): Ctrl+Z / Ctrl+Y, the Edit menu and the top bar's
// buttons across a drop, a drag, an inspector slider and an edge trim. Each user action undoes
// and redoes as one step, in order, and the timeline and inspector follow.
import type { ElectronApplication, Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import {
  chooseMenu,
  clickClip,
  clipAt,
  createProject,
  drag,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  importWithButton,
  inspector,
  item,
  laneY,
  launchFlowApp,
  layout,
  sequence,
  shortcut,
  snapshot,
  timelineGeometry,
  toolbar,
  type Workspace,
  workspace,
  xAt,
} from "./helpers.ts";

/** A project with the interview on Video 1 at 1 s, dropped from the bin; returns its id. */
async function oneClip(app: ElectronApplication, page: Page, work: Workspace): Promise<string> {
  await createProject(app, page, "Undo", work.dir);
  await importWithButton(app, page, [work.file("interview.mp4")]);
  await dropCard(page, "interview.mp4", "V1", 1);
  await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
  await editorCaughtUp(page);
  return clipAt(await timelineGeometry(page), "V1", 1).id;
}

test.describe("F7 undo and redo", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("each action undoes and redoes as one step, with the keys and the buttons", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      const undo = toolbar(page).getByRole("button", { name: /^Undo/ });
      const redo = toolbar(page).getByRole("button", { name: /^Redo/ });
      const f = inspector(page);
      const clips = async () => (await layout(page))[0]?.clips ?? [];
      const opacity = async () => {
        const id = (await sequence(page)).tracks[0]?.items[0]?.id;
        return id === undefined ? undefined : (await item(page, id)).visual.opacity;
      };

      // 1. Drop the interview at 1 s.
      const id = await oneClip(app, page, work);
      await expect(undo).toHaveAccessibleName("Undo Insert clip");
      // Zoom out (- twice on the timeline; view state, not an undo step) so the moved clip's end
      // stays in view.
      await clickClip(page, id);
      await page.keyboard.press("-");
      await page.keyboard.press("-");
      await expect.poll(async () => (await timelineGeometry(page)).pxPerSec).toBeCloseTo(64, 6);
      // 2. Drag it to 3 s.
      let g = await timelineGeometry(page);
      const px = (s: number) => s * g.pxPerSec;
      await drag(
        page,
        { x: xAt(g, clipAt(g, "V1", 1).start) + px(1), y: laneY(g, "V1") },
        { x: xAt(g, clipAt(g, "V1", 1).start) + px(3), y: laneY(g, "V1") },
      );
      await expect.poll(clips).toEqual([[3, 10]]);
      await expect(undo).toHaveAccessibleName("Undo Move clip");
      // 3. One step down on the inspector's opacity slider.
      await f.opacity.focus();
      await page.keyboard.press("ArrowLeft");
      await expect.poll(opacity).toBe(0.99);
      await expect(undo).toHaveAccessibleName("Undo Change opacity");
      // 4. Trim it to 5 s by dragging its right edge.
      g = await timelineGeometry(page);
      const end = xAt(g, clipAt(g, "V1", 3).start + clipAt(g, "V1", 3).duration);
      await drag(
        page,
        { x: end - 2, y: laneY(g, "V1") },
        { x: end - 2 - px(5), y: laneY(g, "V1") },
      );
      await expect.poll(clips).toEqual([[3, 5]]);
      await expect(undo).toHaveAccessibleName("Undo Trim clip");
      await expect(redo).toBeDisabled();
      await snapshot(page, info, "before undo");

      // Ctrl+Z four times: trim, opacity, move, insert come off in that order.
      await shortcut(app, "Control+Z");
      await expect.poll(clips).toEqual([[3, 10]]);
      await expect(f.duration).toHaveValue("00:00:10:00");
      await expect(undo).toHaveAccessibleName("Undo Change opacity");
      await expect(redo).toHaveAccessibleName("Redo Trim clip");
      await shortcut(app, "Control+Z");
      await expect.poll(opacity).toBe(1);
      await expect(f.opacity).toHaveAttribute("aria-valuetext", "100 %");
      await shortcut(app, "Control+Z");
      await expect.poll(clips).toEqual([[1, 10]]);
      await expect(f.start).toHaveValue("00:00:01:00");
      // The fourth from the menu: Edit > Undo.
      await chooseMenu(page, app, "Edit", "Undo");
      await expect.poll(clips).toEqual([]);
      await expect(page.getByText("Nothing selected")).toBeVisible();
      await expect(undo).toHaveAccessibleName("Undo Import media");
      await expect(redo).toHaveAccessibleName("Redo Insert clip");

      // Ctrl+Y, then Edit > Redo: the insert and the move come back.
      await shortcut(app, "Control+Y");
      await expect.poll(clips).toEqual([[1, 10]]);
      await chooseMenu(page, app, "Edit", "Redo");
      await expect.poll(clips).toEqual([[3, 10]]);
      await expect(redo).toHaveAccessibleName("Redo Change opacity");
      // The Redo button twice: the opacity and the trim.
      await redo.click();
      await expect.poll(opacity).toBe(0.99);
      await redo.click();
      await expect.poll(clips).toEqual([[3, 5]]);
      await expect(redo).toBeDisabled();
      // The Undo button once: the trim again.
      await undo.click();
      await expect.poll(clips).toEqual([[3, 10]]);
      await expect(undo).toHaveAccessibleName("Undo Change opacity");
      expect(await opacity()).toBe(0.99);
      // The audio followed every step, and the clip kept its id throughout.
      expect((await layout(page))[1]).toEqual({ track: "A1", clips: [[3, 10]] });
      expect((await sequence(page)).tracks[0]?.items[0]?.id).toBe(id);
      await snapshot(page, info, "after undo and redo");
    } finally {
      await close();
      work.remove();
    }
  });

  test("Ctrl+Z right after typing a value in the inspector undoes that edit", async () => {
    // App bug found by this flow (WP-1O.05): after a value is typed into an inspector field and
    // committed, the first Ctrl+Z does nothing the person can see, even with the focus back on the
    // timeline. Chromium's own text-editing undo (one undo stack per page) takes the key press to
    // undo the typing in the field, so the native menu's accelerator never fires; only a second
    // Ctrl+Z undoes the edit. Owner: packages/editor (WP-1M.07 shortcut wiring).
    test.fixme(true, "the first Ctrl+Z after typing in a field is swallowed (see the note)");
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      const id = await oneClip(app, page, work);
      await clickClip(page, id);
      const f = inspector(page);
      await f.name.click();
      await page.keyboard.type("Take 2");
      await page.keyboard.press("Enter");
      await expect.poll(async () => (await item(page, id)).name).toBe("Take 2");
      // Back on the timeline, one Ctrl+Z.
      await clickClip(page, id);
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await item(page, id)).name).toBeNull();
      await expect(f.name).toHaveValue("");
    } finally {
      await close();
      work.remove();
    }
  });
});
