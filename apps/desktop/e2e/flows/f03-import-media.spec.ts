// SPDX-License-Identifier: GPL-3.0-or-later
// F3 Import media (docs/user-flows.md): Import in the media bin, the OS picker returns the demo
// files; Grid/List; the asset tabs that overflow the panel scroll. The engine probes with the dev
// stub (`--dev-stub-media`) until WP-1I.05: every video reads as 10 s, 1920×1080, 30 fps, and
// the inspector says the data is made up.
import { readFileSync } from "node:fs";
import { expect, test } from "../fixtures.ts";
import {
  chooseMenu,
  createProject,
  DEMO_FILES,
  dirtyDot,
  hasRealEngine,
  launchFlowApp,
  mediaCard,
  mediaCards,
  mediaNames,
  pickFiles,
  snapshot,
  toast,
  workspace,
} from "./helpers.ts";

test.describe("F3 import media", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("Import adds the picked files as cards; Ctrl+I imports more; Grid and List; tabs scroll", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      const doc = await createProject(app, page, "Imports", work.dir);
      await expect(page.getByText("No media yet")).toBeVisible();

      // Import: the OS picker returns four of the demo files.
      const asked = await pickFiles(app, [
        work.file("interview.mp4"),
        work.file("broll-city.mp4"),
        work.file("music.wav"),
        work.file("logo.png"),
      ]);
      await page.getByRole("button", { name: "Import", exact: true }).click();
      await expect(toast(page, "Import finished")).toContainText("4 files added.");
      await expect(mediaCards(page)).toHaveCount(4);
      expect((await asked())[0]?.properties).toEqual(["openFile", "multiSelections"]);

      // Each card shows its name, kind, duration, frame size and rate.
      const interview = mediaCard(page, "interview.mp4");
      await expect(interview).toContainText("Video");
      await expect(interview).toContainText("0:10");
      await expect(interview).toContainText("1920×1080");
      await expect(interview).toContainText("30 fps");
      await expect(mediaCard(page, "music.wav")).toContainText("Audio");
      await expect(mediaCard(page, "music.wav")).toContainText("0:10");
      await expect(mediaCard(page, "logo.png")).toContainText("Image");
      await expect(mediaCard(page, "logo.png")).not.toContainText("0:10");
      await expect(dirtyDot(page)).toBeVisible();
      await snapshot(page, info, "imported (grid)");

      // Ctrl+I opens the picker from anywhere: one new file and one already in the project.
      await pickFiles(app, [work.file("broll-coffee.mp4"), work.file("interview.mp4")]);
      await page.keyboard.press("Control+I");
      await expect(toast(page, "Import finished").last()).toContainText("1 file added.");
      await expect(toast(page, "Import finished").last()).toContainText(
        "1 was already in the project.",
      );
      await expect(mediaCards(page)).toHaveCount(5);

      // A picker the person cancels imports nothing.
      await pickFiles(app, null);
      await page.getByRole("button", { name: "Import", exact: true }).click();
      await expect(page.getByRole("button", { name: "Import", exact: true })).toBeEnabled();
      await expect(mediaCards(page)).toHaveCount(5);

      // Selecting a card shows what the probe found in the inspector.
      await interview.click();
      await expect(interview).toHaveAttribute("aria-selected", "true");
      const media = page.getByTestId("media-inspector");
      await expect(media).toContainText("interview.mp4");
      await expect(media).toContainText("1920 × 1080");
      await expect(media).toContainText("Placeholder data.");

      // Grid and List.
      const bin = page.getByRole("listbox", { name: "Media files" });
      await expect(bin).toHaveAttribute("data-layout", "grid");
      await page.getByRole("radio", { name: "List" }).click();
      await expect(bin).toHaveAttribute("data-layout", "list");
      await expect(mediaCards(page)).toHaveCount(5);
      await snapshot(page, info, "imported (list)");
      await page.getByRole("radio", { name: "Grid" }).click();
      await expect(bin).toHaveAttribute("data-layout", "grid");

      // The asset tabs overflow the panel: the chevron scrolls the hidden ones into view.
      const aiStudio = page.getByRole("tab", { name: "AI Studio" });
      await expect(aiStudio).not.toBeInViewport({ ratio: 1 });
      const next = page.locator(".cw-tabsbar__scroll--end");
      // Each click pages most of the strip's width; click until the last tab is fully shown.
      await expect(async () => {
        if (await next.isVisible()) await next.click();
        await expect(aiStudio).toBeInViewport({ ratio: 1, timeout: 1_000 });
      }).toPass();
      await aiStudio.click();
      await expect(page.getByRole("tabpanel", { name: "AI Studio" })).toContainText(
        "Generate clips, images and voices.",
      );
      await expect(page.locator(".cw-tabsbar__scroll--start")).toBeVisible();
      await page.locator(".cw-tabsbar__scroll--start").click();
      await page.getByRole("tab", { name: "Media" }).click();
      await expect(mediaCards(page)).toHaveCount(5);

      // The files are in the project, and on disk once saved (File > Save).
      expect(await mediaNames(page)).toEqual([...DEMO_FILES].sort());
      await chooseMenu(page, app, "File", "Save");
      await expect(dirtyDot(page)).toBeHidden();
      const saved = JSON.parse(readFileSync(doc, "utf8")) as { media: Record<string, unknown> };
      expect(Object.keys(saved.media)).toHaveLength(5);
    } finally {
      await close();
      work.remove();
    }
  });
});
