// SPDX-License-Identifier: GPL-3.0-or-later
// F42 Context menus (docs/user-flows.md, WP-2H.12): right-click a text field in the Inspector and
// choose Paste; press Shift+F10 and then the Menu key on the focused field and see the menu open
// at it, with Escape returning the focus; right-click dead space and see nothing. Real right
// clicks and keys; the clipboard is the OS's, filled the way a copy in another app would.
// WP-2H.13 extends this flow with the surfaces it adds.
import { expect, test } from "../fixtures.ts";
import {
  clickClip,
  clipAt,
  createProject,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  importWithButton,
  inspector,
  launchFlowApp,
  layout,
  snapshot,
  timelineGeometry,
  timelineTools,
  workspace,
} from "./helpers.ts";

test.describe("F42 context menus", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("a text field has Paste, the keyboard opens a menu at the focused control, dead space has none", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Menus", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
      await editorCaughtUp(page);
      await clickClip(page, clipAt(await timelineGeometry(page), "V1", 1).id);
      const { name } = inspector(page);
      const menu = page.getByRole("menu");

      // Dead space: a right-click on the empty end of the timeline's tool strip shows nothing.
      const strip = await timelineTools(page).boundingBox();
      if (strip === null) throw new Error("no timeline tool strip");
      await page.mouse.click(strip.x + strip.width - 6, strip.y + strip.height / 2, {
        button: "right",
      });
      await expect(menu).toHaveCount(0);

      // A text field: right-click offers the text actions; Paste puts the clipboard in the field.
      await app.evaluate(({ clipboard }, text) => clipboard.writeText(text), "Pasted title");
      await name.click();
      await page.keyboard.press("Control+A");
      await name.click({ button: "right" });
      const fieldMenu = page.getByRole("menu", { name: "Text actions for Name" });
      await expect(fieldMenu).toBeVisible();
      await expect(fieldMenu.getByRole("menuitem")).toHaveText([
        "Undo",
        "Cut",
        "Copy",
        "Paste",
        "Select all",
      ]);
      await snapshot(page, info, "text field menu");
      await fieldMenu.getByRole("menuitem", { name: "Paste" }).click();
      await expect(menu).toHaveCount(0);
      await expect(name).toHaveValue("Pasted title");
      await expect(name).toBeFocused();
      await page.keyboard.press("Escape"); // puts the old name back
      await expect(name).not.toHaveValue("Pasted title");

      // The keyboard: Shift+F10 and the Menu key open it at the focused field; Escape gives the
      // focus back to the field.
      for (const key of ["Shift+F10", "ContextMenu"]) {
        await name.focus();
        await page.keyboard.press(key);
        await expect(page.getByRole("menu", { name: "Text actions for Name" })).toBeVisible();
        await page.keyboard.press("Escape");
        await expect(menu).toHaveCount(0);
        await expect(name).toBeFocused();
      }
    } finally {
      await close();
      work.remove();
    }
  });
});
