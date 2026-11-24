// SPDX-License-Identifier: GPL-3.0-or-later
// F1 First launch (docs/user-flows.md): start the app and see the editor ready to work.
import { editorCall, expect, test } from "../fixtures.ts";
import {
  dirtyDot,
  engineChip,
  hasRealEngine,
  launchFlowApp,
  logProblems,
  snapshot,
  toolbar,
  workspace,
} from "./helpers.ts";

test.describe("F1 first launch", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("the editor opens on an empty untitled project with the engine connected", async () => {
    const info = test.info();
    const work = workspace();
    const { page, mainLog, close } = await launchFlowApp(work);
    try {
      // The four panels of the layout, each with its empty state.
      for (const panel of ["Assets", "Preview", "Inspector", "Timeline"]) {
        await expect(page.locator(".cw-dock-tab", { hasText: panel })).toBeVisible();
      }
      await expect(page.getByText("No media yet")).toBeVisible();
      await expect(page.getByText("Nothing to preview")).toBeVisible();
      await expect(page.getByText("Nothing selected")).toBeVisible();
      await expect(page.getByText("Your timeline is empty")).toBeVisible();

      // Connected to the engine, on an untitled project with nothing to save or undo.
      await expect(engineChip(page)).toHaveText("Engine connected");
      await expect(page).toHaveTitle("Untitled — Cutweave");
      await expect(dirtyDot(page)).toBeHidden();
      await expect(toolbar(page).getByRole("button", { name: "Undo" })).toBeDisabled();
      await expect(toolbar(page).getByRole("button", { name: "Redo" })).toBeDisabled();
      await expect(page.getByRole("button", { name: "Import", exact: true })).toBeEnabled();
      await snapshot(page, info, "first launch");

      const project = await editorCall<{ media_count: number; sequences: unknown[] }>(
        page,
        "project.info",
      );
      expect(project.media_count).toBe(0);
      expect(project.sequences).toEqual([]);
      // Nothing went wrong on the way up.
      expect(logProblems(mainLog)).toEqual([]);
    } finally {
      await close();
      work.remove();
    }
  });
});
