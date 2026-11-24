// SPDX-License-Identifier: GPL-3.0-or-later
// F2 New project (docs/user-flows.md): File > New Project (Ctrl+N), name it, pick a folder.
// The folder picker is the OS dialog: it returns the folder a person would pick.
import { existsSync, readFileSync } from "node:fs";
import { expect, test } from "../fixtures.ts";
import {
  dirtyDot,
  hasRealEngine,
  launchFlowApp,
  namingDialog,
  pickFiles,
  projectDoc,
  sequence,
  shortcut,
  snapshot,
  showRecent,
  workspace,
} from "./helpers.ts";

test.describe("F2 new project", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("Ctrl+N, a name and a folder make a saved project with its folder on disk", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      const asked = await pickFiles(app, [work.dir]);
      await shortcut(app, "Control+N");
      const naming = namingDialog(page);
      await expect(naming.dialog).toBeVisible();
      await expect(naming.name).toBeFocused();

      // Nothing typed yet: the dialog says what is missing and stays open.
      await naming.create.click();
      await expect(naming.dialog.getByRole("alert")).toHaveText("Give the project a name.");
      // A name that cannot be a folder name is refused too.
      await naming.name.click();
      await page.keyboard.type("Coffee/promo");
      await naming.create.click();
      await expect(naming.dialog.getByRole("alert")).toContainText("Names cannot contain");
      await naming.name.fill("");
      await naming.name.pressSequentially("Coffee promo");
      await naming.create.click();
      await expect(naming.dialog.getByRole("alert")).toHaveText(
        "Choose the folder to create it in.",
      );

      // Choose folder… opens the OS folder picker; the chosen folder fills the location.
      await naming.choose.click();
      await expect(naming.location).toHaveValue(work.dir);
      expect((await asked())[0]?.properties).toContain("openDirectory");
      await expect(naming.dialog).toContainText("Cutweave creates a folder named Coffee promo");
      await snapshot(page, info, "naming dialog filled");
      await naming.create.click();

      await expect(naming.dialog).toBeHidden();
      await expect(page).toHaveTitle("Coffee promo — Cutweave");
      await expect(dirtyDot(page)).toBeHidden();
      // The project folder and its document exist, with the new sequence's two tracks.
      const doc = projectDoc(work.dir, "Coffee promo");
      expect(existsSync(doc)).toBe(true);
      expect(JSON.parse(readFileSync(doc, "utf8")).meta.name).toBe("Coffee promo");
      expect((await sequence(page)).tracks.map((t) => t.name)).toEqual(["V1", "A1"]);
      await expect(page.getByText("Your timeline is empty")).toBeHidden();

      // It is on the Recent list.
      const recent = await showRecent(page);
      await expect(recent.getByRole("menuitem", { name: "Coffee promo.cutweave" })).toBeVisible();
      await page.keyboard.press("Escape");
      await page.keyboard.press("Escape");
      await snapshot(page, info, "new project");
    } finally {
      await close();
      work.remove();
    }
  });

  test("Cancel in the dialog leaves the current project as it was", async () => {
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await shortcut(app, "Control+N");
      const naming = namingDialog(page);
      await expect(naming.dialog).toBeVisible();
      await naming.name.pressSequentially("Never made");
      await naming.cancel.click();
      await expect(naming.dialog).toBeHidden();
      await expect(page).toHaveTitle("Untitled — Cutweave");
      expect(existsSync(projectDoc(work.dir, "Never made"))).toBe(false);
    } finally {
      await close();
      work.remove();
    }
  });
});
