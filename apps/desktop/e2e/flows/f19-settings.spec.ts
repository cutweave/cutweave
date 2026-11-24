// SPDX-License-Identifier: GPL-3.0-or-later
// F19 Settings (docs/user-flows.md; WP-1M.11), as a person does it: open Settings with Ctrl+, and
// from the File menu; search "auto"; change the theme and "On startup"; reset a row; export the
// settings to a file; Reset all; import the file; look at the folders; quit and relaunch. The
// export defaults and the shortcut list are WP-1M.13's part of the flow.
import { readFileSync } from "node:fs";
import path from "node:path";
import type { ElectronApplication, Locator, Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import {
  chooseMenu,
  closeSettings,
  createProject,
  hasRealEngine,
  launchFlowApp,
  openSettings,
  pickFiles,
  pickSavePath,
  profileDir,
  removeDir,
  shortcut,
  snapshot,
  workspace,
} from "./helpers.ts";

declare global {
  /** What the stubbed `shell.openPath` was asked to show (installed by `stubOpenPath`). */
  var __cutweaveFlowOpened: string[] | undefined;
}

const theme = (page: Page) => page.evaluate(() => document.documentElement.dataset.theme);
const frameColor = (app: ElectronApplication) =>
  app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.getBackgroundColor());

/** The OS file manager is not ours: `shell.openPath` records what it is asked to show. */
async function stubOpenPath(app: ElectronApplication): Promise<() => Promise<string[]>> {
  await app.evaluate(({ shell }) => {
    globalThis.__cutweaveFlowOpened = [];
    shell.openPath = async (target: string) => {
      globalThis.__cutweaveFlowOpened?.push(target);
      return "";
    };
  });
  return () => app.evaluate(() => globalThis.__cutweaveFlowOpened ?? []);
}

/** Waits for the dialog's opening animation to end, so a screenshot shows it as it rests. */
const opaque = (dialog: Locator): Promise<void> =>
  expect.poll(() => dialog.evaluate((el) => getComputedStyle(el).opacity)).toBe("1");

const radio = (group: Locator, name: string): Locator => group.getByRole("radio", { name });

test.describe("F19 settings", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("the frame, search, theme, On startup, reset, export, Reset all, import, and a relaunch", async () => {
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    const exported = path.join(work.dir, "my-settings.json");
    try {
      const first = await launchFlowApp(work, home);
      const { app, page } = first;
      // A project to come back to when "On startup" asks for the last one.
      await createProject(app, page, "Startup", work.dir);
      expect(await theme(page)).toBe("dark");
      const darkFrame = await frameColor(app);

      // From File > Settings…: the page list, grouped; General is open; the search field has focus.
      let settings = await openSettings(page, app);
      const pages = settings.getByRole("tablist", { name: "Settings pages" });
      await expect(pages.getByRole("tab")).toHaveText([
        "General",
        "Appearance",
        "Saving and backups",
        "Notifications",
        "Agents and MCP",
        "Advanced",
      ]);
      await expect(settings.getByText("Preferences", { exact: true })).toBeVisible();
      await expect(pages.getByRole("tab", { name: "General" })).toHaveAttribute(
        "aria-selected",
        "true",
      );
      const search = settings.getByRole("searchbox", { name: "Search settings" });
      await expect(search).toBeFocused();
      await opaque(settings);
      await snapshot(page, info, "settings general dark");
      await closeSettings(settings);

      // And with Ctrl+, .
      await shortcut(app, "Control+,");
      settings = page.getByRole("dialog", { name: "Settings" });
      await expect(settings).toBeVisible();

      // Search "auto": the Saving and backups rows show, with the match marked; Esc clears it
      // and General is back; a search with no match says so.
      await search.fill("auto");
      const results = settings.getByRole("region", { name: "Search results" });
      const saving = results.getByRole("region", { name: "Saving and backups" });
      await expect(saving.getByRole("switch", { name: /Save\s?auto\s?matically/ })).toBeVisible();
      await expect(saving.getByRole("radiogroup", { name: "Save at least every" })).toBeVisible();
      await expect(saving.getByRole("radiogroup", { name: "Backups to keep" })).toBeVisible();
      await expect(saving.locator("mark").first()).toHaveText(/auto/i);
      await expect(settings.getByRole("heading", { name: "Results for “auto”" })).toBeVisible();
      await opaque(settings);
      await snapshot(page, info, "settings search auto");
      await search.fill("zzzz");
      await expect(settings.getByText("No settings match “zzzz”")).toBeVisible();
      await snapshot(page, info, "settings search none");
      await search.fill("auto");
      await page.keyboard.press("Escape");
      await expect(search).toHaveValue("");
      await expect(settings).toBeVisible();
      await expect(pages.getByRole("tab", { name: "General" })).toHaveAttribute(
        "aria-selected",
        "true",
      );

      // The theme, on the Appearance page: Light applies at once.
      await pages.getByRole("tab", { name: "Appearance" }).click();
      const themeChoice = settings.getByRole("radiogroup", { name: "Theme" });
      await radio(themeChoice, "Light").click();
      await expect.poll(() => theme(page)).toBe("light");
      await opaque(settings);
      await snapshot(page, info, "settings appearance light");

      // On startup: open the last project. Reset that row and see its default; set it again.
      await pages.getByRole("tab", { name: "General" }).click();
      const startup = settings.getByRole("radiogroup", { name: "On startup" });
      await expect(radio(startup, "Empty project")).toHaveAttribute("aria-checked", "true");
      await radio(startup, "Last project").click();
      await expect(radio(startup, "Last project")).toHaveAttribute("aria-checked", "true");
      await settings.getByRole("button", { name: "Reset On startup" }).click();
      await expect(radio(startup, "Empty project")).toHaveAttribute("aria-checked", "true");
      await expect(settings.getByRole("button", { name: "Reset On startup" })).toHaveCount(0);

      // Export: the OS save dialog is stubbed. The file holds the changed setting, nothing else.
      const proposed = await pickSavePath(app, exported);
      await pages.getByRole("tab", { name: "Advanced" }).click();
      await settings.getByRole("button", { name: "Export settings…" }).click();
      await expect(settings.getByText(`Settings exported to ${exported}.`)).toBeVisible();
      expect(path.basename((await proposed())[0] ?? "")).toBe("cutweave-settings.json");
      expect(JSON.parse(readFileSync(exported, "utf8"))).toEqual({
        version: 1,
        app: { "appearance.theme": "light" },
      });

      // Reset all settings… asks first; Cancel changes nothing; Reset all puts the defaults back.
      await settings.getByRole("button", { name: "Reset all settings…" }).click();
      const confirm = page.getByRole("dialog", { name: "Reset all settings?" });
      await expect(confirm).toBeVisible();
      await confirm.getByRole("button", { name: "Cancel" }).click();
      await expect(confirm).toBeHidden();
      expect(await theme(page)).toBe("light");
      await settings.getByRole("button", { name: "Reset all settings…" }).click();
      await confirm.getByRole("button", { name: "Reset all settings" }).click();
      await expect(confirm).toBeHidden();
      await expect.poll(() => theme(page)).toBe("dark");
      await expect(settings.getByText("All settings are back to their defaults.")).toBeVisible();

      // Import the file: the theme comes back.
      await pickFiles(app, [exported]);
      await settings.getByRole("button", { name: "Import settings…" }).click();
      await expect(settings.getByText("Settings imported: 1.")).toBeVisible();
      await expect.poll(() => theme(page)).toBe("light");

      // A file that is not settings (a picture) is refused, and says so.
      await pickFiles(app, [work.file("logo.png")]);
      await settings.getByRole("button", { name: "Import settings…" }).click();
      await expect(settings.getByText("That file could not be read.")).toBeVisible();

      // The folders open in the OS file manager (stubbed): the settings and the logs.
      const opened = await stubOpenPath(app);
      await settings.getByRole("button", { name: "Open settings folder" }).click();
      await expect.poll(async () => (await opened()).length).toBe(1);
      await settings.getByRole("button", { name: "Open logs folder" }).click();
      await expect.poll(async () => (await opened()).length).toBe(2);
      const [settingsFolder, logsFolder] = await opened();
      expect(settingsFolder).toBe(path.join(home, "userData"));
      expect(path.basename(logsFolder ?? "")).toBe("logs");

      // On startup: the last project, for the relaunch.
      await pages.getByRole("tab", { name: "General" }).click();
      await radio(startup, "Last project").click();
      await expect(radio(startup, "Last project")).toHaveAttribute("aria-checked", "true");
      await closeSettings(settings);

      // Relaunch on the same profile: the values persist, the window opens in the chosen theme
      // and, as asked, on the last project.
      const closed = app.waitForEvent("close");
      await chooseMenu(page, app, "File", "Exit");
      await closed;
      await first.close();
      const second = await launchFlowApp(work, home);
      try {
        const page2 = second.page;
        expect(await theme(page2)).toBe("light");
        expect(await frameColor(second.app)).not.toBe(darkFrame);
        await expect(page2).toHaveTitle("Startup — Cutweave");
        const again = await openSettings(page2, second.app);
        await expect(
          radio(again.getByRole("radiogroup", { name: "On startup" }), "Last project"),
        ).toHaveAttribute("aria-checked", "true");
        await again.getByRole("tab", { name: "Appearance" }).click();
        await expect(
          radio(again.getByRole("radiogroup", { name: "Theme" }), "Light"),
        ).toHaveAttribute("aria-checked", "true");
        await snapshot(page2, info, "settings after relaunch");

        // Tidy up as a person would: everything back to the defaults.
        await again.getByRole("tab", { name: "Advanced" }).click();
        await again.getByRole("button", { name: "Reset all settings…" }).click();
        await page2
          .getByRole("dialog", { name: "Reset all settings?" })
          .getByRole("button", { name: "Reset all settings" })
          .click();
        await expect.poll(() => theme(page2)).toBe("dark");
        await closeSettings(again);
      } finally {
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
