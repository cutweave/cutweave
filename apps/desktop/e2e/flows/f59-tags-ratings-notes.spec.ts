// SPDX-License-Identifier: GPL-3.0-or-later
// F59 Tags, ratings and notes in the bin (docs/user-flows.md; WP-2F.06b, ADR-0036): tag three
// cards at once from the card's menu; tag one more from its menu and from the media info, where a
// suggestion is one click; rate from the media info's stars and from the card's Set rating ▸;
// write a note; filter the bin by tag and by rating and find the note's words in the search; undo
// each step and redo them; save, quit and reopen: the tags, the ratings and the note are there.
import type { Locator, Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import {
  chooseMenu,
  createProject,
  DEMO_FILES,
  dirtyDot,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  mediaCard,
  mediaCards,
  openRecent,
  profileDir,
  removeDir,
  shortcut,
  snapshot,
  toolbar,
  workspace,
} from "./helpers.ts";

const NOTE = "Second take is the good one.";

interface Marks {
  tags: string[];
  rating: number | null;
  note: string | null;
}

/** Every asset's tags, rating and note as the engine has them, by file name. */
async function engineMarks(page: Page): Promise<Record<string, Marks>> {
  const out = await editorCall<{
    media: { name: string; tags: string[]; rating: number | null; note?: string | null }[];
  }>(page, "media.list", { include: ["note"] });
  return Object.fromEntries(
    out.media
      .map((m): [string, Marks] => [
        m.name,
        { tags: m.tags, rating: m.rating, note: m.note ?? null },
      ])
      .sort((a, b) => a[0].localeCompare(b[0])),
  );
}

const NONE: Marks = { tags: [], rating: null, note: null };
/** What the flow has made by the time it saves. */
const FINAL: Record<string, Marks> = {
  "broll-city.mp4": { tags: ["city", "keep"], rating: null, note: null },
  "broll-coffee.mp4": { tags: ["keep"], rating: 5, note: null },
  "interview.mp4": { tags: ["city", "keep"], rating: 4, note: NOTE },
  "logo.png": NONE,
  "music.wav": NONE,
};

const cardTags = (page: Page, name: string): Locator =>
  mediaCard(page, name).locator(".cw-media__tag");
const cardRating = (page: Page, name: string): Locator =>
  mediaCard(page, name).locator(".cw-media__rating");
const info = (page: Page): Locator => page.getByTestId("media-organize");
const chips = (scope: Locator): Locator =>
  scope.getByRole("list", { name: "Tags" }).getByRole("listitem");

/** Right-clicks a card and chooses a row of its menu. */
async function cardMenu(page: Page, card: string, row: string): Promise<void> {
  await mediaCard(page, card).click({ button: "right" });
  await page.getByRole("menuitem", { name: row, exact: true }).click();
}

/** Opens a submenu of the bin's Filters menu. */
async function filtersSubmenu(page: Page, name: "Tags" | "Rating"): Promise<Locator> {
  await page.getByRole("button", { name: /^Filters/ }).click();
  await page.getByRole("menuitem", { name, exact: true }).click();
  const menu = page.getByRole("menu", { name });
  await expect(menu).toBeVisible();
  return menu;
}

test.describe("F59 tags, ratings and notes", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("tag three at once, rate, note, filter, undo each, save and reopen", async () => {
    test.setTimeout(300_000);
    const shots = test.info();
    const work = workspace();
    const home = profileDir();
    try {
      const first = await launchFlowApp(work, home);
      const { app, page } = first;
      await createProject(app, page, "Tags", work.dir);
      await importWithButton(
        app,
        page,
        DEMO_FILES.map((f) => work.file(f)),
      );
      // The list has room for a card's tags; the grid shows only its rating.
      await page.getByRole("radio", { name: "List" }).click();
      const undo = toolbar(page).getByRole("button", { name: /^Undo/ });
      const redo = toolbar(page).getByRole("button", { name: /^Redo/ });

      // 1. Three cards selected, Add tag… on one of them: the tag goes on all three, one step.
      await mediaCard(page, "broll-city.mp4").click();
      await mediaCard(page, "broll-coffee.mp4").click({ modifiers: ["ControlOrMeta"] });
      await mediaCard(page, "interview.mp4").click({ modifiers: ["ControlOrMeta"] });
      await cardMenu(page, "interview.mp4", "Add tag…");
      const three = page.getByRole("dialog", { name: "Tags of 3 items" });
      await expect(three).toBeVisible();
      await expect(three.getByRole("combobox", { name: "Add tag" })).toBeFocused();
      await page.keyboard.type("keep");
      await page.keyboard.press("Enter");
      await expect(chips(three)).toHaveCount(1);
      await expect(chips(three)).toContainText(["keep"]);
      await three.getByRole("button", { name: "Done" }).click();
      await expect(three).toBeHidden();
      for (const name of ["broll-city.mp4", "broll-coffee.mp4", "interview.mp4"]) {
        await expect(cardTags(page, name)).toHaveText(["keep"]);
      }
      await expect(cardTags(page, "music.wav")).toHaveCount(0);
      await expect(undo).toHaveAccessibleName("Undo Add tag");

      // 2. One card's own menu: a comma adds the tag too.
      await mediaCard(page, "broll-city.mp4").click();
      await cardMenu(page, "broll-city.mp4", "Add tag…");
      const one = page.getByRole("dialog", { name: "Tags of broll-city.mp4" });
      await expect(one).toBeVisible();
      await page.keyboard.type("city,");
      await expect(chips(one)).toContainText(["city", "keep"]);
      await expect(one.getByRole("combobox", { name: "Add tag" })).toHaveValue("");
      await one.getByRole("button", { name: "Done" }).click();
      await expect(cardTags(page, "broll-city.mp4")).toHaveText(["city", "keep"]);

      // With the three selected again the editor says which tag all have and which only one has.
      await mediaCard(page, "broll-coffee.mp4").click({ modifiers: ["ControlOrMeta"] });
      await mediaCard(page, "interview.mp4").click({ modifiers: ["ControlOrMeta"] });
      await cardMenu(page, "interview.mp4", "Add tag…");
      await expect(three).toBeVisible();
      const partial = chips(three).filter({ hasText: "city" });
      await expect(partial).toHaveAttribute("data-partial", "true");
      await expect(partial).toContainText("1 of 3");
      await expect(chips(three).filter({ hasText: "keep" })).not.toHaveAttribute("data-partial");
      await snapshot(page, shots, "tags of three, one partial");
      await three.getByRole("button", { name: "Done" }).click();
      await expect(three).toBeHidden();

      // 3. The media info of one card: stars, the tag field with a suggestion, and the note.
      await mediaCard(page, "interview.mp4").click();
      await expect(info(page)).toBeVisible();
      await info(page).getByRole("radio", { name: "4 stars" }).click();
      await expect(info(page).getByRole("radio", { name: "4 stars" })).toBeChecked();
      await expect(cardRating(page, "interview.mp4")).toHaveText("4");
      await expect(undo).toHaveAccessibleName("Undo Set rating");

      await info(page).getByRole("combobox", { name: "Add tag" }).click();
      await page.keyboard.type("ci");
      const suggested = page.getByRole("listbox", { name: "Suggested tags" });
      await expect(suggested.getByRole("option")).toHaveText(["city"]);
      await suggested.getByRole("option", { name: "city" }).click();
      await expect(chips(info(page))).toContainText(["city", "keep"]);
      await expect(cardTags(page, "interview.mp4")).toHaveText(["city", "keep"]);

      const note = info(page).getByRole("textbox", { name: "Note" });
      await note.click();
      await page.keyboard.type(NOTE);
      await page.keyboard.press("Control+Enter");
      await expect.poll(async () => (await engineMarks(page))["interview.mp4"]?.note).toBe(NOTE);
      await expect(undo).toHaveAccessibleName("Undo Edit note");

      // 4. Set rating ▸ on another card's menu.
      await cardMenu(page, "broll-coffee.mp4", "Set rating");
      const ratingMenu = page.getByRole("menu", { name: "Set rating" });
      await expect(ratingMenu.getByRole("menuitemradio", { name: "None" })).toBeChecked();
      await ratingMenu.getByRole("menuitemradio", { name: "5 stars" }).click();
      await expect(cardRating(page, "broll-coffee.mp4")).toHaveText("5");
      await expect.poll(() => engineMarks(page)).toEqual(FINAL);
      await snapshot(page, shots, "tagged and rated");

      // 5. Filter by tag, then by rating too; the search reads the note.
      const tagsMenu = await filtersSubmenu(page, "Tags");
      await expect(tagsMenu.getByRole("menuitemcheckbox")).toHaveText(["city", "keep"]);
      await tagsMenu.getByRole("menuitemcheckbox", { name: "keep" }).click();
      await page.keyboard.press("Escape");
      await page.keyboard.press("Escape");
      await expect(mediaCards(page)).toHaveCount(3);
      const active = page.getByRole("group", { name: "Active filters" });
      await expect(active).toContainText("Tag: keep");
      await expect(page.getByText("3 of 5", { exact: true })).toBeVisible();

      const stars = await filtersSubmenu(page, "Rating");
      await stars.getByRole("menuitemradio", { name: "4 stars or more" }).click();
      await page.keyboard.press("Escape");
      await page.keyboard.press("Escape");
      await expect(mediaCards(page)).toHaveCount(2);
      await expect(mediaCard(page, "interview.mp4")).toBeVisible();
      await expect(mediaCard(page, "broll-coffee.mp4")).toBeVisible();
      await expect(page.getByRole("button", { name: "Filters, 2 active" })).toBeVisible();
      await expect(active).toContainText("4 stars or more");
      await snapshot(page, shots, "filtered by tag and rating");

      await page.getByRole("button", { name: "Remove filter Tag: keep" }).click();
      await expect(mediaCards(page)).toHaveCount(2);
      await active.getByRole("button", { name: "Clear all" }).click();
      await expect(mediaCards(page)).toHaveCount(5);
      const search = page.getByRole("searchbox", { name: "Search media" });
      await search.click();
      await page.keyboard.type("second take");
      await expect(mediaCards(page)).toHaveCount(1);
      await expect(mediaCard(page, "interview.mp4")).toBeVisible();
      await page.getByRole("button", { name: "Clear search" }).click();
      await expect(mediaCards(page)).toHaveCount(5);

      // 6. Undo each step, newest first, then redo them all.
      await undo.click();
      await expect(cardRating(page, "broll-coffee.mp4")).toHaveCount(0);
      await undo.click();
      await expect.poll(async () => (await engineMarks(page))["interview.mp4"]?.note).toBeNull();
      await undo.click();
      await expect(cardTags(page, "interview.mp4")).toHaveText(["keep"]);
      await undo.click();
      await expect(cardRating(page, "interview.mp4")).toHaveCount(0);
      await undo.click();
      await expect(cardTags(page, "broll-city.mp4")).toHaveText(["keep"]);
      await expect(undo).toHaveAccessibleName("Undo Add tag");
      await undo.click();
      await expect(page.locator(".cw-media__tag")).toHaveCount(0);
      await expect
        .poll(async () => Object.values(await engineMarks(page)))
        .toEqual([NONE, NONE, NONE, NONE, NONE]);
      await expect(undo).toHaveAccessibleName("Undo Import media");
      for (let i = 0; i < 6; i++) await redo.click();
      await expect.poll(() => engineMarks(page)).toEqual(FINAL);
      await expect(cardTags(page, "interview.mp4")).toHaveText(["city", "keep"]);

      // 7. Save, quit and reopen: everything is in the project file.
      await shortcut(app, "Control+S");
      await expect(dirtyDot(page)).toBeHidden();
      const closed = app.waitForEvent("close");
      await chooseMenu(page, app, "File", "Exit");
      await closed;
      await first.close();

      const second = await launchFlowApp(work, home);
      try {
        const again = second.page;
        await openRecent(again, "Tags.cutweave");
        await expect(again).toHaveTitle("Tags — Cutweave");
        await expect(mediaCards(again)).toHaveCount(5);
        await again.getByRole("radio", { name: "List" }).click();
        await expect.poll(() => engineMarks(again)).toEqual(FINAL);
        await expect(cardTags(again, "interview.mp4")).toHaveText(["city", "keep"]);
        await expect(cardTags(again, "broll-coffee.mp4")).toHaveText(["keep"]);
        await expect(cardRating(again, "interview.mp4")).toHaveText("4");
        await expect(cardRating(again, "broll-coffee.mp4")).toHaveText("5");
        await mediaCard(again, "interview.mp4").click();
        await expect(info(again).getByRole("radio", { name: "4 stars" })).toBeChecked();
        await expect(chips(info(again))).toContainText(["city", "keep"]);
        await expect(info(again).getByRole("textbox", { name: "Note" })).toHaveValue(NOTE);
        const listed = await editorCall<{ tags: { tag: string; count: number }[] }>(
          again,
          "media.list_tags",
          {},
        );
        expect(listed.tags).toEqual([
          { tag: "city", count: 2 },
          { tag: "keep", count: 3 },
        ]);
        await snapshot(again, shots, "reopened");
      } finally {
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
