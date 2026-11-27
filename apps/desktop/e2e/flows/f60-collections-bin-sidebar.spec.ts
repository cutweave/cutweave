// SPDX-License-Identifier: GPL-3.0-or-later
// F60 Collections and the bin sidebar (docs/user-flows.md; WP-2F.06c, ADR-0036): tag three cards,
// filter the bin by the tag and save that as a smart collection; the sidebar lists it with its
// count, and it stays current when a fourth card gets the tag; make a manual collection from a
// card's Add to collection ▸ New collection…, drag another card onto its row, open it (stored
// order) and take a card off it again; rename a collection from its row's menu; update the smart
// collection from the current search; delete the manual collection (the dialog says the media
// stay) and undo; save, quit and reopen: the collections are in the sidebar, as the engine has
// them. The folder tree's drop target is flow F22.
import type { Locator, Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import {
  binSidebar,
  chooseMenu,
  createProject,
  DEMO_FILES,
  dirtyDot,
  dragTileToSidebar,
  hasRealEngine,
  hideBinSidebar,
  importWithButton,
  launchFlowApp,
  mediaCard,
  mediaCards,
  openRecent,
  profileDir,
  removeDir,
  shortcut,
  showBinSidebar,
  snapshot,
  toast,
  toolbar,
  workspace,
} from "./helpers.ts";

interface Listed {
  name: string;
  kind: "manual" | "smart";
  filter: Record<string, unknown> | null;
  media_count: number;
}

/** The collections as the engine has them: name, kind, filter and member count, by name. */
async function engineCollections(page: Page): Promise<Listed[]> {
  const out = await editorCall<{ collections: (Listed & { id: string })[] }>(
    page,
    "media.list_collections",
    {},
  );
  return out.collections.map(({ name, kind, filter, media_count }) => ({
    name,
    kind,
    filter: filter === null ? null : stripEmpty(filter),
    media_count,
  }));
}

/** A filter without the parts that are absent (the engine answers them as null). */
function stripEmpty(filter: Record<string, unknown>): Record<string, unknown> {
  return Object.fromEntries(Object.entries(filter).filter(([, value]) => value !== null));
}

const names = (page: Page) => mediaCards(page).locator(".cw-media__name").allTextContents();

/** The row of a collection in the sidebar, by its name. */
const collectionRow = (side: Locator, name: string): Locator =>
  side
    .getByRole("tree", { name: "Collections" })
    .getByRole("treeitem")
    .filter({ has: side.page().getByText(name, { exact: true }) });

/** Right-clicks a card and chooses a row of its menu. */
async function cardMenu(page: Page, card: string, row: string): Promise<void> {
  await mediaCard(page, card).click({ button: "right" });
  await page.getByRole("menuitem", { name: row, exact: true }).click();
}

/** Right-clicks a collection's row in the sidebar and chooses a row of its menu. */
async function collectionMenu(page: Page, name: string, row: string): Promise<void> {
  const side = await showBinSidebar(page);
  await collectionRow(side, name).click({ button: "right" });
  const menu = page.getByRole("menu", { name: `Collection actions for ${name}` });
  await expect(menu).toBeVisible();
  await menu.getByRole("menuitem", { name: row, exact: true }).click();
}

const KEEP = ["broll-city.mp4", "broll-coffee.mp4", "interview.mp4"];

test.describe("F60 collections and the bin sidebar", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("save a search as a smart collection, build a manual one, reopen", async () => {
    test.setTimeout(300_000);
    const shots = test.info();
    const work = workspace();
    const home = profileDir();
    try {
      const first = await launchFlowApp(work, home);
      const { app, page } = first;
      await createProject(app, page, "Collections", work.dir);
      await importWithButton(
        app,
        page,
        DEMO_FILES.map((f) => work.file(f)),
      );
      // The list layout keeps every card in view of the small panel.
      await page.getByRole("radio", { name: "List" }).click();
      const undo = toolbar(page).getByRole("button", { name: /^Undo/ });

      // 1. Tag three cards at once.
      await mediaCard(page, "broll-city.mp4").click();
      await mediaCard(page, "broll-coffee.mp4").click({ modifiers: ["ControlOrMeta"] });
      await mediaCard(page, "interview.mp4").click({ modifiers: ["ControlOrMeta"] });
      await cardMenu(page, "interview.mp4", "Add tag…");
      const tags = page.getByRole("dialog", { name: "Tags of 3 items" });
      await expect(tags.getByRole("combobox", { name: "Add tag" })).toBeFocused();
      await page.keyboard.type("keep");
      await page.keyboard.press("Enter");
      await tags.getByRole("button", { name: "Done" }).click();
      await expect(tags).toBeHidden();
      await expect(page.locator(".cw-media__tag")).toHaveCount(3);

      // 2. Filter by the tag: the three cards, and the offer to save the search.
      await page.getByRole("button", { name: /^Filters/ }).click();
      await page.getByRole("menuitem", { name: "Tags", exact: true }).click();
      await page
        .getByRole("menu", { name: "Tags" })
        .getByRole("menuitemcheckbox", { name: "keep" })
        .click();
      await page.keyboard.press("Escape");
      await page.keyboard.press("Escape");
      await expect(mediaCards(page)).toHaveCount(3);
      const active = page.getByRole("group", { name: "Active filters" });
      await expect(active).toContainText("Tag: keep");

      // 3. Save as smart collection…: a name, starting from the tag.
      await active.getByRole("button", { name: "Save as smart collection…" }).click();
      const save = page.getByRole("dialog", { name: "Save as smart collection" });
      const nameField = save.getByRole("textbox", { name: "Collection name" });
      await expect(nameField).toBeFocused();
      await expect(nameField).toHaveValue("keep");
      await page.keyboard.type("Keepers");
      await snapshot(page, shots, "save as smart collection");
      await page.keyboard.press("Enter");
      await expect(save).toBeHidden();
      await expect(
        toast(page, "Saved Keepers. It is in the sidebar of the media bin."),
      ).toBeVisible();
      await expect(undo).toHaveAccessibleName("Undo New collection");
      await expect
        .poll(() => engineCollections(page))
        .toEqual([{ name: "Keepers", kind: "smart", filter: { tags: ["keep"] }, media_count: 3 }]);

      // 4. The sidebar lists it under Collections, with its count.
      await active.getByRole("button", { name: "Clear all" }).click();
      await expect(mediaCards(page)).toHaveCount(5);
      let side = await showBinSidebar(page);
      await expect(side.getByRole("tree", { name: "Folders" }).getByRole("treeitem")).toHaveText([
        "All media",
      ]);
      await expect(collectionRow(side, "Keepers")).toHaveAccessibleName(
        "Keepers, smart collection, 3 items",
      );
      await snapshot(page, shots, "sidebar with the smart collection");

      // Opening it shows its members; it is the marked place, and the breadcrumb names it.
      await collectionRow(side, "Keepers").click();
      await expect.poll(async () => (await names(page)).sort()).toEqual(KEEP);
      await expect(
        page.getByRole("navigation", { name: "Media folders" }).getByRole("button"),
      ).toHaveText(["Media", "Keepers"]);
      side = await showBinSidebar(page);
      await expect(collectionRow(side, "Keepers")).toHaveAttribute("aria-selected", "true");

      // 5. It stays current: a fourth card gets the tag and is on it at once.
      await side.getByRole("treeitem", { name: "All media" }).click();
      await expect(mediaCards(page)).toHaveCount(5);
      await mediaCard(page, "music.wav").click();
      await cardMenu(page, "music.wav", "Add tag…");
      const one = page.getByRole("dialog", { name: "Tags of music.wav" });
      await expect(one.getByRole("combobox", { name: "Add tag" })).toBeFocused();
      await page.keyboard.type("keep");
      await page.keyboard.press("Enter");
      await one.getByRole("button", { name: "Done" }).click();
      await expect(one).toBeHidden();
      side = await showBinSidebar(page);
      await expect(collectionRow(side, "Keepers")).toHaveAccessibleName(
        "Keepers, smart collection, 4 items",
      );

      // 6. A manual collection from a card's menu: Add to collection ▸ New collection…
      await hideBinSidebar(page);
      await mediaCard(page, "logo.png").click();
      await mediaCard(page, "logo.png").click({ button: "right" });
      await page.getByRole("menuitem", { name: "Add to collection", exact: true }).click();
      const addTo = page.getByRole("menu", { name: "Add to collection" });
      await expect(addTo.getByRole("menuitem")).toHaveText(["New collection…"]);
      await addTo.getByRole("menuitem", { name: "New collection…" }).click();
      const create = page.getByRole("dialog", { name: "New collection" });
      await expect(create).toContainText("The selected item goes into it and stays in its folder.");
      await expect(create.getByRole("textbox", { name: "Collection name" })).toBeFocused();
      await page.keyboard.type("Graphics");
      await page.keyboard.press("Enter");
      await expect(create).toBeHidden();
      side = await showBinSidebar(page);
      await expect(collectionRow(side, "Graphics")).toHaveAccessibleName(
        "Graphics, collection, 1 item",
      );

      // A card dragged onto its row is added; the card stays where it is.
      await dragTileToSidebar(mediaCard(page, "interview.mp4"), collectionRow(side, "Graphics"));
      await expect(toast(page, "Added 1 item to Graphics")).toBeVisible();
      await expect(undo).toHaveAccessibleName("Undo Add to collection");
      await expect(mediaCard(page, "interview.mp4")).toBeVisible();
      side = await showBinSidebar(page);
      await expect(collectionRow(side, "Graphics")).toHaveAccessibleName(
        "Graphics, collection, 2 items",
      );
      // The menu lists it now, and says a card that is on it already is.
      await hideBinSidebar(page);
      await mediaCard(page, "broll-city.mp4").click();
      await mediaCard(page, "broll-city.mp4").click({ button: "right" });
      await page.getByRole("menuitem", { name: "Add to collection", exact: true }).click();
      await expect(addTo.getByRole("menuitem")).toHaveText(["New collection…", "Graphics"]);
      await addTo.getByRole("menuitem", { name: "Graphics" }).click();
      await expect(toast(page, "Added 1 item to Graphics")).toBeVisible();

      // 7. Open it: the members in the order they were added. Remove one from the collection.
      side = await showBinSidebar(page);
      await collectionRow(side, "Graphics").click();
      await expect.poll(() => names(page)).toEqual(["logo.png", "interview.mp4", "broll-city.mp4"]);
      await snapshot(page, shots, "manual collection open");
      await mediaCard(page, "interview.mp4").click();
      await cardMenu(page, "interview.mp4", "Remove from collection");
      await expect.poll(() => names(page)).toEqual(["logo.png", "broll-city.mp4"]);
      await expect(undo).toHaveAccessibleName("Undo Remove from collection");

      // 8. Rename it from its row's menu, in place.
      await collectionMenu(page, "Graphics", "Rename");
      const rename = binSidebar(page).getByRole("textbox", { name: "Collection name" });
      await expect(rename).toBeFocused();
      await rename.fill("Titles");
      await page.keyboard.press("Enter");
      await expect(rename).toBeHidden();
      await expect(collectionRow(binSidebar(page), "Titles")).toBeVisible();
      await expect(undo).toHaveAccessibleName("Undo Rename collection");

      // 9. Update the smart collection from the current search: back in All media, search, then
      // its row's menu.
      await binSidebar(page).getByRole("treeitem", { name: "All media" }).click();
      const search = page.getByRole("searchbox", { name: "Search media" });
      await search.fill("broll");
      await expect(mediaCards(page)).toHaveCount(2);
      await collectionMenu(page, "Keepers", "Update from current filters");
      await expect(toast(page, "Updated Keepers from the current filters")).toBeVisible();
      await expect(collectionRow(binSidebar(page), "Keepers")).toHaveAccessibleName(
        "Keepers, smart collection, 2 items",
      );
      await page.getByRole("button", { name: "Clear search" }).click();
      await expect(mediaCards(page)).toHaveCount(5);

      const FINAL: Listed[] = [
        { name: "Keepers", kind: "smart", filter: { text: "broll" }, media_count: 2 },
        { name: "Titles", kind: "manual", filter: null, media_count: 2 },
      ];
      await expect.poll(() => engineCollections(page)).toEqual(FINAL);

      // 10. Delete the manual collection: it lists something, so it asks, and says the media stay.
      await collectionMenu(page, "Titles", "Delete collection…");
      const remove = page.getByRole("dialog", { name: "Delete the collection Titles?" });
      await expect(remove).toContainText("It lists 2 items.");
      await expect(remove).toContainText("The media stay in the project and in their folders.");
      await snapshot(page, shots, "delete collection dialog");
      await remove.getByRole("button", { name: "Delete collection" }).click();
      await expect(remove).toBeHidden();
      await expect(collectionRow(binSidebar(page), "Titles")).toHaveCount(0);
      await expect(mediaCards(page)).toHaveCount(5);
      await expect(undo).toHaveAccessibleName("Undo Delete collection");
      // Undo brings it back with its members.
      await undo.click();
      await expect(collectionRow(binSidebar(page), "Titles")).toHaveAccessibleName(
        "Titles, collection, 2 items",
      );
      await expect.poll(() => engineCollections(page)).toEqual(FINAL);

      // 11. Save, quit and reopen: the collections are in the sidebar.
      await shortcut(app, "Control+S");
      await expect(dirtyDot(page)).toBeHidden();
      const closed = app.waitForEvent("close");
      await chooseMenu(page, app, "File", "Exit");
      await closed;
      await first.close();

      const second = await launchFlowApp(work, home);
      try {
        const again = second.page;
        await openRecent(again, "Collections.cutweave");
        await expect(again).toHaveTitle("Collections — Cutweave");
        await expect(mediaCards(again)).toHaveCount(5);
        const reopened = await showBinSidebar(again);
        await expect(
          reopened.getByRole("tree", { name: "Collections" }).getByRole("treeitem"),
        ).toHaveCount(2);
        await expect(collectionRow(reopened, "Keepers")).toHaveAccessibleName(
          "Keepers, smart collection, 2 items",
        );
        await expect(collectionRow(reopened, "Titles")).toHaveAccessibleName(
          "Titles, collection, 2 items",
        );
        await expect.poll(() => engineCollections(again)).toEqual(FINAL);
        await collectionRow(reopened, "Titles").click();
        await again.getByRole("radio", { name: "List" }).click();
        await expect.poll(() => names(again)).toEqual(["logo.png", "broll-city.mp4"]);
        await snapshot(again, shots, "reopened");
      } finally {
        await second.close();
      }
    } finally {
      removeDir(work.dir);
      removeDir(home);
    }
  });
});
