// SPDX-License-Identifier: GPL-3.0-or-later
// F43 Command palette (docs/user-flows.md, WP-2H.04): open it with Ctrl+K, type, choose with
// Enter and see the effect: Split at playhead on the timeline, Undo, a panel toggle, and a
// Settings page. Keyboard only, as a person who never reaches for the mouse would.
import { expect, test } from "../fixtures.ts";
import {
  clickClip,
  clipAt,
  createProject,
  dropCard,
  editorCaughtUp,
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
  workspace,
  xAt,
} from "./helpers.ts";

test.describe("F43 command palette", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("Ctrl+K, type, Enter: split at the playhead, undo, hide a panel, open a settings page", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Palette", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
      await editorCaughtUp(page);
      await clickClip(page, clipAt(await timelineGeometry(page), "V1", 1).id);
      // The playhead inside the clip, at 6 s.
      const g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(6)), rulerY(g));
      await expect(timecode(page)).toHaveText("00:00:06:00");

      const palette = page.getByRole("dialog", { name: "Command palette" });
      const search = palette.getByRole("combobox", { name: "Search commands" });
      const options = palette.getByRole("option");
      const open = async (query: string): Promise<void> => {
        await shortcut(app, "Control+K");
        await expect(palette).toBeVisible();
        await expect(search).toBeFocused();
        await page.keyboard.type(query);
      };

      // Split at playhead: the best match is the first row, with its key; Enter splits.
      await open("split at playhead");
      await expect(options.first()).toContainText("Split at playhead");
      await snapshot(page, info, "palette with a query");
      await page.keyboard.press("Enter");
      await expect(palette).toHaveCount(0);
      await expect
        .poll(() => layout(page))
        .toEqual([
          {
            track: "V1",
            clips: [
              [1, 5],
              [6, 5],
            ],
          },
          {
            track: "A1",
            clips: [
              [1, 5],
              [6, 5],
            ],
          },
        ]);
      await editorCaughtUp(page);

      // Undo, by name.
      await open("undo");
      await expect(options.first()).toContainText("Undo");
      await page.keyboard.press("Enter");
      await expect(palette).toHaveCount(0);
      await expect
        .poll(() => layout(page))
        .toEqual([
          { track: "V1", clips: [[1, 10]] },
          { track: "A1", clips: [[1, 10]] },
        ]);

      // A panel: hide the Inspector, then bring it back the same way.
      const inspectorPanel = page.getByRole("tabpanel", { name: "Inspector", exact: true });
      await expect(inspectorPanel).toBeVisible();
      await open("inspector panel");
      await expect(options.first()).toContainText("Show or hide the Inspector panel");
      await page.keyboard.press("Enter");
      await expect(inspectorPanel).toHaveCount(0);
      await open("inspector panel");
      await page.keyboard.press("Enter");
      await expect(inspectorPanel).toBeVisible();

      // Recent commands come first, and Esc closes an empty palette.
      await shortcut(app, "Control+K");
      await expect(palette.getByRole("group", { name: "Recent" })).toBeVisible();
      await expect(options.first()).toContainText("Show or hide the Inspector panel");
      await page.keyboard.press("Escape");
      await expect(palette).toHaveCount(0);

      // A settings page, opened from the palette.
      await open("keyboard shortcuts");
      await expect(options.first()).toContainText("Settings");
      await page.keyboard.press("Enter");
      const settings = page.getByRole("dialog", { name: "Settings" });
      await expect(settings).toBeVisible();
      await expect(settings.getByRole("heading", { name: "Keyboard shortcuts" })).toBeVisible();
      await settings.getByRole("button", { name: "Done" }).click();
      await expect(settings).toHaveCount(0);
    } finally {
      await close();
    }
  });
});
