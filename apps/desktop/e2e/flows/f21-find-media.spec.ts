// SPDX-License-Identifier: GPL-3.0-or-later
// F21 Find media (docs/user-flows.md): the bin's search, filters and sort. Type part of a name:
// only matching cards remain and the toolbar counts them; Down and Enter insert the first result
// at the playhead; the Filters menu (Audio, then Not used) narrows with chips and a badge; sort by
// name; a search that matches nothing shows the empty state and "Clear search" brings everything
// back.
import { expect, test } from "../fixtures.ts";
import {
  clipNames,
  createProject,
  DEMO_FILES,
  editorCaughtUp,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  mediaCard,
  mediaCards,
  snapshot,
  workspace,
} from "./helpers.ts";

test.describe("F21 find media", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("search by name, filter by kind and use, sort, no match, clear, Down and Enter", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Find", work.dir);
      const search = () => page.getByRole("searchbox", { name: "Search media" });
      // Before any media there is nothing to search: the empty state stays as it was.
      await expect(page.getByText("No media yet")).toBeVisible();
      await expect(search()).toBeHidden();

      await importWithButton(
        app,
        page,
        DEMO_FILES.map((f) => work.file(f)),
      );
      await expect(mediaCards(page)).toHaveCount(5);
      const names = () => mediaCards(page).locator(".cw-media__name").allTextContents();

      // Type part of a name: only the matches remain, and the toolbar says how many.
      await search().click();
      await page.keyboard.type("BROLL");
      await expect(mediaCards(page)).toHaveCount(2);
      await expect(page.getByText("2 of 5", { exact: true })).toBeVisible();
      await expect(mediaCard(page, "broll-city.mp4")).toBeVisible();
      await expect(mediaCard(page, "broll-coffee.mp4")).toBeVisible();
      await snapshot(page, info, "search broll");

      // Esc clears the text and keeps the focus; the count goes away with the search.
      await page.keyboard.press("Escape");
      await expect(search()).toHaveValue("");
      await expect(search()).toBeFocused();
      await expect(mediaCards(page)).toHaveCount(5);
      await expect(page.getByText("2 of 5", { exact: true })).toBeHidden();

      // A narrower text, then Down moves to the first result and Enter inserts it at the
      // playhead: the video and its linked audio land on V1 and A1.
      await page.keyboard.type("city");
      await expect(mediaCards(page)).toHaveCount(1);
      await page.keyboard.press("ArrowDown");
      await expect(mediaCard(page, "broll-city.mp4")).toBeFocused();
      await expect(mediaCard(page, "broll-city.mp4")).toHaveAttribute("aria-selected", "true");
      await page.keyboard.press("Enter");
      await expect.poll(() => clipNames(page, "V1")).toEqual(["broll-city.mp4"]);
      await expect.poll(() => clipNames(page, "A1")).toEqual(["broll-city.mp4"]);
      await editorCaughtUp(page);

      // The selected card stays selected when a search hides it.
      await page.getByRole("button", { name: "Clear search" }).click();
      await expect(mediaCards(page)).toHaveCount(5);
      await mediaCard(page, "interview.mp4").click();
      await expect(mediaCard(page, "interview.mp4")).toHaveAttribute("aria-selected", "true");
      await search().click();
      await page.keyboard.type("music");
      await expect(mediaCards(page)).toHaveCount(1);
      await expect(mediaCard(page, "music.wav")).toBeVisible();
      await page.getByRole("button", { name: "Clear search" }).click();
      await expect(mediaCards(page)).toHaveCount(5);
      await expect(mediaCard(page, "interview.mp4")).toHaveAttribute("aria-selected", "true");

      // Filters: Not used drops the clip that is on the timeline; adding Audio leaves the music.
      await page.getByRole("button", { name: "Filters", exact: true }).click();
      await page.getByRole("menuitemradio", { name: "Not used" }).click();
      await page.getByRole("menuitemcheckbox", { name: "Audio" }).click();
      // An open menu hides the page behind it from the accessibility tree: close it to read.
      await page.keyboard.press("Escape");
      await expect(mediaCards(page)).toHaveCount(1);
      await expect(page.getByRole("button", { name: "Filters, 2 active" })).toBeVisible();
      const chips = page.getByRole("group", { name: "Active filters" });
      await expect(chips).toContainText("Audio");
      await expect(chips).toContainText("Not used");
      await expect(page.getByText("1 of 5", { exact: true })).toBeVisible();
      await expect(mediaCard(page, "music.wav")).toBeVisible();
      await snapshot(page, info, "filters audio, not used");

      // A chip's x removes that filter alone; Clear all removes the rest.
      await page.getByRole("button", { name: "Remove filter Audio" }).click();
      await expect(mediaCards(page)).toHaveCount(4);
      await chips.getByRole("button", { name: "Clear all" }).click();
      await expect(mediaCards(page)).toHaveCount(5);
      await expect(chips).toBeHidden();

      // Sort by name.
      await page.getByRole("button", { name: "Sort", exact: true }).click();
      await page.getByRole("menuitemradio", { name: "Name, A to Z" }).click();
      await expect
        .poll(names)
        .toEqual(["broll-city.mp4", "broll-coffee.mp4", "interview.mp4", "logo.png", "music.wav"]);
      await page.getByRole("button", { name: "Sort", exact: true }).click();
      await page.getByRole("menuitemradio", { name: "Longest first" }).click();
      await expect.poll(async () => (await names()).at(-1)).toBe("logo.png");

      // Nothing matches: the empty state names the text, and Clear search brings everything back.
      await search().click();
      await page.keyboard.type("no such clip");
      await expect(mediaCards(page)).toHaveCount(0);
      const empty = page.locator(".cw-media .cw-empty");
      await expect(empty).toContainText('No media matches "no such clip"');
      await snapshot(page, info, "no match");
      await empty.getByRole("button", { name: "Clear search" }).click();
      await expect(mediaCards(page)).toHaveCount(5);
      await expect(search()).toHaveValue("");

      // Ctrl+F from inside the bin goes to the search field.
      await mediaCard(page, "music.wav").click();
      await page.keyboard.press("Control+F");
      await expect(search()).toBeFocused();
    } finally {
      await close();
      work.remove();
    }
  });
});
