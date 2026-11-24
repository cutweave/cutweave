// SPDX-License-Identifier: GPL-3.0-or-later
// F10 Unsaved changes on New/Open (docs/user-flows.md): edit, then File > New or Open, and answer
// the same Save / Discard / Cancel dialog as on quit. Cancel stays; Discard leaves the edit out
// of the file (the journal keeps it, so reopening offers it back); Save writes it first. The
// folder picker of Open is the OS dialog: it returns the project a person would pick.
import { readFileSync } from "node:fs";
import path from "node:path";
import { expect, test, unsavedPrompt } from "../fixtures.ts";
import {
  chooseMenu,
  createProject,
  dirtyDot,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  mediaCards,
  namingDialog,
  openRecent,
  pickFiles,
  recoveryDialog,
  shortcut,
  snapshot,
  workspace,
} from "./helpers.ts";

const mediaOnDisk = (doc: string): number =>
  Object.keys((JSON.parse(readFileSync(doc, "utf8")) as { media: object }).media).length;

test.describe("F10 unsaved changes on New/Open", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("New: Cancel keeps the edit; Discard leaves it out of the file and reopening offers it back", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      const first = await createProject(app, page, "First", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await expect(dirtyDot(page)).toBeVisible();

      // Ctrl+N asks first; Cancel stays on the project with its edit, and nothing is created.
      await shortcut(app, "Control+N");
      const prompt = unsavedPrompt(page);
      await expect(prompt.dialog).toHaveAccessibleName("Save changes to First?");
      await snapshot(page, info, "save changes prompt");
      await prompt.cancel.click();
      await expect(prompt.dialog).toBeHidden();
      await expect(namingDialog(page).dialog).toBeHidden();
      await expect(page).toHaveTitle("• First — Cutweave");
      await expect(mediaCards(page)).toHaveCount(1);

      // File > New Project… this time, and Discard: the naming dialog follows, and the new
      // project opens empty.
      await chooseMenu(page, app, "File", "New project…");
      await prompt.discard.click();
      const naming = namingDialog(page);
      await expect(naming.dialog).toBeVisible();
      await naming.name.fill("Second");
      await naming.location.fill(work.dir);
      await naming.create.click();
      await expect(page).toHaveTitle("Second — Cutweave");
      await expect(page.getByText("No media yet")).toBeVisible();
      expect(mediaOnDisk(first)).toBe(0);

      // The discarded edit stayed in First's journal: reopening it offers it back.
      await openRecent(page, "First.cutweave");
      const recovery = recoveryDialog(page);
      await expect(recovery.dialog).toBeVisible();
      await expect(recovery.dialog).toContainText("1 edit");
      await recovery.discard.click();
      await expect(recovery.dialog).toBeHidden();
      await expect(page).toHaveTitle("First — Cutweave");
      await expect(page.getByText("No media yet")).toBeVisible();
      await expect(dirtyDot(page)).toBeHidden();
    } finally {
      await close();
      work.remove();
    }
  });

  test("Open: the picked project opens after Save wrote the edit to the current one", async () => {
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Other", work.dir);
      const current = await createProject(app, page, "Current", work.dir);
      await importWithButton(app, page, [work.file("music.wav")]);
      await expect(dirtyDot(page)).toBeVisible();

      // File > Open Project…: the OS picker returns Other's folder, then the editor asks about
      // Current.
      await pickFiles(app, [path.join(work.dir, "Other")]);
      await chooseMenu(page, app, "File", "Open project…");
      const prompt = unsavedPrompt(page);
      await expect(prompt.dialog).toHaveAccessibleName("Save changes to Current?");
      await prompt.save.click();
      await expect(page).toHaveTitle("Other — Cutweave");
      await expect(dirtyDot(page)).toBeHidden();
      expect(mediaOnDisk(current)).toBe(1);

      // Back to Current from the top bar's Open button: no question (nothing unsaved), no
      // recovery offer (it was saved), and the music is there.
      await pickFiles(app, [path.join(work.dir, "Current")]);
      await page.getByRole("button", { name: "Open", exact: true }).click();
      await expect(page).toHaveTitle("Current — Cutweave");
      await expect(prompt.dialog).toBeHidden();
      await expect(recoveryDialog(page).dialog).toBeHidden();
      await expect(mediaCards(page)).toHaveCount(1);
    } finally {
      await close();
      work.remove();
    }
  });

  test("Open: Cancel in the dialog stays on the current project", async () => {
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Elsewhere", work.dir);
      await createProject(app, page, "Here", work.dir);
      await importWithButton(app, page, [work.file("logo.png")]);
      await expect(dirtyDot(page)).toBeVisible();
      await pickFiles(app, [path.join(work.dir, "Elsewhere")]);
      await shortcut(app, "Control+O");
      const prompt = unsavedPrompt(page);
      await expect(prompt.dialog).toHaveAccessibleName("Save changes to Here?");
      await prompt.cancel.click();
      await expect(prompt.dialog).toBeHidden();
      await expect(page).toHaveTitle("• Here — Cutweave");
      await expect(mediaCards(page)).toHaveCount(1);
    } finally {
      await close();
      work.remove();
    }
  });
});
