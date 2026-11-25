// SPDX-License-Identifier: GPL-3.0-or-later
// F22 Organize media in folders (docs/user-flows.md, WP-2F.08): create "Interviews" and inside it
// "Day 1" (through the bin's context menu), rename it "Day one"; Ctrl-click three clips and drag
// them onto Interviews, then onto "Day one"; open it with a double-click and see them; go up with
// the breadcrumb; drag one clip back to the top level by dropping it on the breadcrumb; search in
// All media (path captions, matching folders first) and in This folder; delete Interviews with
// Keep media (through its context menu) and see its contents lifted; Ctrl+Z brings the folder
// back; save, relaunch and reopen: the folders are there, and the bin opens where it was left.
// Importing OS folders and tags are WP-2F.09 and WP-2F.06 and extend this flow later.
import type { Locator, Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import {
  chooseMenu,
  createProject,
  dirtyDot,
  editorCaughtUp,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  mediaCards,
  openRecent,
  pickFiles,
  profileDir,
  removeDir,
  shortcut,
  snapshot,
  toast,
  workspace,
} from "./helpers.ts";

/** A tile of the bin, folder or asset, by the name it shows. */
const tile = (page: Page, name: string): Locator =>
  mediaCards(page).filter({ has: page.getByText(name, { exact: true }) });
const names = (page: Page) =>
  mediaCards(page).locator(".cw-media__name, .cw-foldertile__name").allTextContents();
const crumbs = (page: Page) => page.getByRole("navigation", { name: "Media folders" });
const crumb = (page: Page, name: string) => crumbs(page).getByRole("button", { name });
const nameField = (page: Page) => page.getByRole("textbox", { name: "Folder name" });

test.describe("F22 organize media in folders", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("create, nest, rename, drag, search, delete with Keep media, undo, relaunch", async () => {
    test.setTimeout(300_000);
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    try {
      const first = await launchFlowApp(work, home);
      const { app, page } = first;
      await createProject(app, page, "Folders", work.dir);
      await importWithButton(
        app,
        page,
        ["interview.mp4", "broll-city.mp4", "broll-coffee.mp4", "music.wav", "logo.png"].map((f) =>
          work.file(f),
        ),
      );
      // The list layout keeps every tile in view of the small panel, so a drag needs no scrolling.
      await page.getByRole("radio", { name: "List" }).click();
      // Without folders the bin looks as it always did: no breadcrumb.
      await expect(crumbs(page)).toBeHidden();

      // New folder (the toolbar button) starts renaming it in place.
      await page.getByRole("button", { name: "New folder", exact: true }).click();
      await expect(nameField(page)).toBeFocused();
      await expect(nameField(page)).toHaveValue("New folder");
      await nameField(page).fill("Interviews");
      await page.keyboard.press("Enter");
      await expect(nameField(page)).toBeHidden();
      await expect(tile(page, "Interviews")).toBeVisible();
      await expect(tile(page, "Interviews")).toHaveAttribute(
        "aria-label",
        "Interviews, folder, empty",
      );
      // Folders come first, then the assets; the breadcrumb is there now.
      expect((await names(page))[0]).toBe("Interviews");
      await expect(crumb(page, "Media")).toHaveAttribute("aria-current", "page");

      // Open it with a double-click: it is empty, and New folder in the empty bin's context menu
      // makes "Day 1" inside it.
      await tile(page, "Interviews").dblclick();
      await expect(crumb(page, "Interviews")).toHaveAttribute("aria-current", "page");
      await expect(page.getByText("This folder is empty")).toBeVisible();
      // Double-clicking the empty body of the panel opens the import dialog (the OS picker is
      // stubbed) and imports into the open folder; a file already in the project is reported.
      await pickFiles(app, [work.file("music.wav")]);
      await page.locator(".cw-media .cw-empty").dblclick();
      await expect(toast(page, "Import finished").first()).toBeVisible();
      await expect(page.getByText("This folder is empty")).toBeVisible();
      await page.locator(".cw-media .cw-empty").click({ button: "right" });
      const menu = page.getByRole("menu", { name: "Media bin actions" });
      await expect(menu).toBeVisible();
      await menu.getByRole("menuitem", { name: /^New folder/ }).click();
      await expect(nameField(page)).toHaveValue("New folder");
      await nameField(page).fill("Day 1");
      await page.keyboard.press("Enter");
      await expect(tile(page, "Day 1")).toBeVisible();

      // F2 renames it in place.
      await tile(page, "Day 1").click();
      await page.keyboard.press("F2");
      await expect(nameField(page)).toBeFocused();
      await nameField(page).fill("Day one");
      await page.keyboard.press("Enter");
      await expect(tile(page, "Day one")).toBeVisible();
      await expect(tile(page, "Day 1")).toBeHidden();
      await editorCaughtUp(page);

      // Back to the top level with the breadcrumb. Ctrl-click three clips and drag them onto
      // Interviews: one move, they leave the top level.
      await crumb(page, "Media").click();
      await expect(tile(page, "interview.mp4")).toBeVisible();
      // An asset gets a display name through its context menu (the file keeps its name).
      await tile(page, "logo.png").click({ button: "right" });
      await page
        .getByRole("menu", { name: "Media actions for logo.png" })
        .getByRole("menuitem", { name: /^Rename/ })
        .click();
      const assetName = page.getByRole("textbox", { name: "Media name" });
      await expect(assetName).toBeFocused();
      await assetName.fill("Brand logo");
      await page.keyboard.press("Enter");
      await expect(tile(page, "Brand logo")).toBeVisible();
      await expect(tile(page, "logo.png")).toBeHidden();
      await tile(page, "interview.mp4").click();
      await tile(page, "broll-city.mp4").click({ modifiers: ["Control"] });
      await tile(page, "broll-coffee.mp4").click({ modifiers: ["Control"] });
      await expect(mediaCards(page).and(page.locator('[aria-selected="true"]'))).toHaveCount(3);
      await tile(page, "broll-coffee.mp4").dragTo(tile(page, "Interviews"));
      await expect(tile(page, "interview.mp4")).toBeHidden();
      await expect(tile(page, "Interviews")).toHaveAttribute(
        "aria-label",
        "Interviews, folder, 4 items",
      );
      await snapshot(page, info, "clips moved into Interviews");

      // Inside Interviews: Day one and the three clips. The same three go onto Day one.
      await tile(page, "Interviews").dblclick();
      await expect(mediaCards(page)).toHaveCount(4);
      expect((await names(page))[0]).toBe("Day one");
      await tile(page, "interview.mp4").click();
      await tile(page, "broll-city.mp4").click({ modifiers: ["Control"] });
      await tile(page, "broll-coffee.mp4").click({ modifiers: ["Control"] });
      await tile(page, "interview.mp4").dragTo(tile(page, "Day one"));
      await expect(tile(page, "interview.mp4")).toBeHidden();

      // Open Day one with a double-click and see them; go up with the breadcrumb.
      await tile(page, "Day one").dblclick();
      await expect(crumb(page, "Day one")).toHaveAttribute("aria-current", "page");
      await expect(mediaCards(page)).toHaveCount(3);
      await expect(tile(page, "interview.mp4")).toBeVisible();
      await snapshot(page, info, "inside Day one");

      // Drag one clip back to the top level by dropping it on the breadcrumb's first segment.
      await tile(page, "interview.mp4").dragTo(crumb(page, "Media"));
      await expect(tile(page, "interview.mp4")).toBeHidden();
      await crumb(page, "Media").click();
      await expect(tile(page, "interview.mp4")).toBeVisible();
      await expect(crumb(page, "Media")).toHaveAttribute("aria-current", "page");

      // Search in All media: results are flat with the path under each name, and a folder whose
      // name matches comes first.
      const search = page.getByRole("searchbox", { name: "Search media" });
      await search.fill("broll");
      await expect(mediaCards(page)).toHaveCount(2);
      await expect(page.locator(".cw-media__path")).toHaveText([
        "Interviews / Day one",
        "Interviews / Day one",
      ]);
      await search.fill("inter");
      await expect.poll(() => names(page)).toEqual(["Interviews", "interview.mp4"]);
      // This folder looks only inside the open folder and below.
      await search.fill("");
      await tile(page, "Interviews").dblclick();
      await search.fill("mp4");
      await expect(mediaCards(page)).toHaveCount(3); // All media: the whole bin
      await page.getByRole("radio", { name: "This folder" }).click();
      await expect(mediaCards(page)).toHaveCount(2); // broll-city, broll-coffee (in Day one)
      await snapshot(page, info, "search in this folder");
      await page.getByRole("button", { name: "Clear search" }).click();
      await page.getByRole("radio", { name: "All media" }).click();
      await crumb(page, "Media").click();

      // Delete Interviews with Keep media, through its context menu: its contents are lifted.
      await tile(page, "Interviews").click({ button: "right" });
      const folderMenu = page.getByRole("menu", { name: "Folder actions for Interviews" });
      await expect(folderMenu).toBeVisible();
      await expect(folderMenu.getByRole("menuitem", { name: /^Open/ })).toBeEnabled();
      await snapshot(page, info, "folder menu");
      await folderMenu.getByRole("menuitem", { name: /^Delete folder/ }).click();
      const dialog = page.getByRole("dialog");
      await expect(dialog).toContainText("Delete “Interviews” and its 2 media?");
      await snapshot(page, info, "delete folder dialog");
      await dialog.getByRole("button", { name: "Keep media" }).click();
      await expect(dialog).toBeHidden();
      await expect(tile(page, "Interviews")).toBeHidden();
      await expect(tile(page, "Day one")).toBeVisible();
      await editorCaughtUp(page);

      // Ctrl+Z brings the folder back, with Day one inside it.
      await shortcut(app, "Control+Z");
      await expect(tile(page, "Interviews")).toBeVisible();
      await expect(tile(page, "Day one")).toBeHidden();
      await tile(page, "Interviews").dblclick();
      await expect(tile(page, "Day one")).toBeVisible();

      // Save, quit and reopen from Recent: the folders are there, and the bin opens inside
      // Interviews, where it was left.
      await shortcut(app, "Control+S");
      await expect(dirtyDot(page)).toBeHidden();
      const closed = app.waitForEvent("close");
      await chooseMenu(page, app, "File", "Exit");
      await closed;
      await first.close();

      const second = await launchFlowApp(work, home);
      try {
        await openRecent(second.page, "Folders.cutweave");
        await expect(second.page).toHaveTitle("Folders — Cutweave");
        await expect(crumb(second.page, "Interviews")).toHaveAttribute("aria-current", "page");
        await expect(tile(second.page, "Day one")).toBeVisible();
        await crumb(second.page, "Media").click();
        await expect(tile(second.page, "Interviews")).toBeVisible();
        await expect(tile(second.page, "interview.mp4")).toBeVisible();
        await expect(tile(second.page, "Brand logo")).toBeVisible();
        await snapshot(second.page, info, "reopened");
      } finally {
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
