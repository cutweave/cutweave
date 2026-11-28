// SPDX-License-Identifier: GPL-3.0-or-later
// F35 Keyboard shortcuts (docs/user-flows.md, WP-2H.05): Settings > Keyboard shortcuts. Rebind
// Split to Ctrl+K and see it split on the timeline; bind it to Ctrl+Z and answer the conflict
// prompt with Cancel; make Ctrl+Shift+Z the Redo the Edit menu shows; export the shortcuts to a
// file (the OS save dialog is the one stub), Reset all shortcuts, import them back; relaunch: the
// bindings persist. A second test switches the preset to CapCut (Ctrl+B splits) and types in the
// inspector's name field, where no shortcut fires. Keys are real key presses, clicks are real clicks.
import { readFileSync } from "node:fs";
import path from "node:path";
import type { Locator, Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import {
  chooseMenu,
  clickClip,
  clipAt,
  closeSettings,
  createProject,
  dropCard,
  editorCaughtUp,
  freeMainTrack,
  hasRealEngine,
  importWithButton,
  inspector,
  launchFlowApp,
  layout,
  openSettingsPage,
  pickFiles,
  pickSavePath,
  profileDir,
  removeDir,
  rulerY,
  sec,
  snapshot,
  timecode,
  timelineGeometry,
  workspace,
  xAt,
} from "./helpers.ts";

const MAC = process.platform === "darwin";
/** The modifier a person presses, and how the page names it. */
const MOD = MAC ? "Meta" : "Control";
const MOD_NAME = MAC ? "⌘" : "Ctrl";
const SPLIT = "Split at the playhead";

const WHOLE = [
  { track: "V1", clips: [[1, 10]] },
  { track: "A1", clips: [[1, 10]] },
];
const SPLIT_IN_TWO = [
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
];

/** A clip on V1 from 1 s, selected, with the playhead at 6 s (inside it). */
async function clipWithPlayhead(
  page: Page,
  app: Parameters<typeof createProject>[0],
  work: ReturnType<typeof workspace>,
  name: string,
): Promise<void> {
  await createProject(app, page, name, work.dir);
  await importWithButton(app, page, [work.file("interview.mp4")]);
  await dropCard(page, "interview.mp4", "V1", 1);
  await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
  await editorCaughtUp(page);
  await selectAndPlace(page);
}

/** Selects the clip and puts the playhead at 6 s: the timeline has the focus afterwards. */
async function selectAndPlace(page: Page): Promise<void> {
  await clickClip(page, clipAt(await timelineGeometry(page), "V1", 1).id);
  const g = await timelineGeometry(page);
  await page.mouse.click(xAt(g, sec(6)), rulerY(g));
  await expect(timecode(page)).toHaveText("00:00:06:00");
}

/** The row's keys: the group named "<command>: <keys>". */
const keysOf = (settings: Locator, name: string, keys: string): Locator =>
  settings.getByRole("group", { name: `${name}: ${keys}`, exact: true });

/** Narrows the page to the commands that match, as a person looks one up. */
async function find(settings: Locator, query: string): Promise<void> {
  await settings.getByRole("searchbox", { name: "Find a shortcut" }).fill(query);
}

/** Presses Add shortcut on a command's row and then the keys. */
async function record(settings: Locator, page: Page, name: string, chord: string): Promise<void> {
  const add = settings.getByRole("button", { name: `Add a shortcut for ${name}`, exact: true });
  // The Add control is an icon button: no text on it, its tooltip says what it does (#169).
  await expect(add).toHaveText("");
  await add.hover();
  await expect(page.getByRole("tooltip").filter({ hasText: "Add shortcut" }).first()).toBeVisible();
  await add.click();
  await expect(add).toHaveAttribute("aria-pressed", "true");
  await expect(settings.getByText("Press the keys…")).toBeVisible();
  await page.keyboard.press(chord);
}

test.describe("F35 keyboard shortcuts", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");
  // The macOS app keeps its native menu, whose accelerators (Cmd+Z, Cmd+S) are fixed and which the
  // page does not take shortcuts from; the flow's Undo and Redo keys are the Windows and Linux ones.
  test.skip(MAC, "the macOS native menu keeps its own accelerators");

  test("rebind Split, answer a conflict, choose the Redo key, export, reset, import, relaunch", async () => {
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    const exported = path.join(work.dir, "my-shortcuts.json");
    try {
      const first = await launchFlowApp(work, home);
      const { app, page } = first;
      // Clips stay where they are dropped; the setting is kept for the relaunch below.
      await freeMainTrack(app, page);
      await clipWithPlayhead(page, app, work, "Keys");

      // The page lists the keys the app has now, and the preset.
      let settings = await openSettingsPage(page, app, "Keyboard shortcuts");
      await expect(
        settings
          .getByRole("radiogroup", { name: "Shortcut preset" })
          .getByRole("radio", { name: "Cutweave (NLE)" }),
      ).toHaveAttribute("aria-checked", "true");
      await find(settings, "split");
      await expect(keysOf(settings, SPLIT, "S")).toBeVisible();

      // Rebind Split to Ctrl+K. The palette has it: the page asks, and Reassign takes it.
      await record(settings, page, SPLIT, `${MOD}+K`);
      const prompt = settings.getByRole("alertdialog", { name: "Shortcut already in use" });
      await expect(prompt).toContainText(`${MOD_NAME}+K already runs Command palette`);
      await snapshot(page, info, "shortcuts conflict prompt");
      await prompt.getByRole("button", { name: "Reassign" }).click();
      await expect(prompt).toBeHidden();
      await expect(keysOf(settings, SPLIT, `${MOD_NAME}+K, S`)).toBeVisible();
      await closeSettings(settings);

      // It splits on the timeline: the clip is in two at the playhead, its sound too.
      await selectAndPlace(page);
      await page.keyboard.press(`${MOD}+K`);
      await expect.poll(() => layout(page)).toEqual(SPLIT_IN_TWO);
      await editorCaughtUp(page);
      // The palette's key is free now: it opens nothing.
      await expect(page.getByRole("dialog", { name: "Command palette" })).toHaveCount(0);
      await chooseMenu(page, app, "Edit", /^Undo/);
      await expect.poll(() => layout(page)).toEqual(WHOLE);

      // Bind Split to Ctrl+Z: Undo has it. Cancel keeps both as they were.
      settings = await openSettingsPage(page, app, "Keyboard shortcuts");
      await find(settings, "split");
      await record(settings, page, SPLIT, `${MOD}+Z`);
      await expect(prompt).toContainText(`${MOD_NAME}+Z already runs Undo`);
      await prompt.getByRole("button", { name: "Cancel" }).click();
      await expect(prompt).toBeHidden();
      await expect(keysOf(settings, SPLIT, `${MOD_NAME}+K, S`)).toBeVisible();
      await find(settings, "undo");
      await expect(keysOf(settings, "Undo", `${MOD_NAME}+Z`)).toBeVisible();

      // A key no command may take is refused, with the reason.
      await find(settings, "split");
      await record(settings, page, SPLIT, "F10");
      await expect(settings.getByRole("alert").filter({ hasText: "F10" })).toHaveText(
        "F10 cannot be used. The menu bar uses this key to open its menus.",
      );

      // Make Ctrl+Shift+Z the Redo key; the Edit menu shows it.
      await find(settings, "redo");
      await expect(keysOf(settings, "Redo", `${MOD_NAME}+Y, ${MOD_NAME}+Shift+Z`)).toBeVisible();
      await record(settings, page, "Redo", `${MOD}+Shift+Z`);
      await expect(keysOf(settings, "Redo", `${MOD_NAME}+Shift+Z, ${MOD_NAME}+Y`)).toBeVisible();
      await closeSettings(settings);
      await page
        .getByRole("menubar", { name: "Application menu" })
        .getByRole("menuitem", { name: "Edit", exact: true })
        .click();
      const edit = page.getByRole("menu", { name: "Edit" });
      await expect(edit.getByRole("menuitem", { name: /^Redo/ })).toContainText(
        `${MOD_NAME}+Shift+Z`,
      );
      await page.keyboard.press("Escape");
      await expect(edit).toBeHidden();

      // Export the shortcuts to a file (the OS save dialog is stubbed): the changes only.
      settings = await openSettingsPage(page, app, "Keyboard shortcuts");
      const proposed = await pickSavePath(app, exported);
      await settings.getByRole("button", { name: "Export shortcuts…" }).click();
      await expect(settings.getByText(`Shortcuts exported to ${exported}.`)).toBeVisible();
      expect(path.basename((await proposed())[0] ?? "")).toBe("cutweave-shortcuts.json");
      expect(JSON.parse(readFileSync(exported, "utf8"))).toEqual({
        version: 1,
        app: {
          "shortcuts.bindings": {
            "clip.split": ["Mod+K", "S"],
            "palette.open": [],
            "edit.redo": ["Mod+Shift+Z", "Mod+Y"],
          },
        },
      });

      // Reset all shortcuts asks first; then Split is S again and Ctrl+K is the palette's.
      await settings.getByRole("button", { name: "Reset all shortcuts" }).click();
      const confirm = page.getByRole("dialog", { name: "Reset all shortcuts?" });
      await expect(confirm).toBeVisible();
      await confirm.getByRole("button", { name: "Reset shortcuts" }).click();
      await expect(confirm).toBeHidden();
      await expect(
        settings.getByText("All shortcuts are back to the Cutweave preset."),
      ).toBeVisible();
      await find(settings, "split");
      await expect(keysOf(settings, SPLIT, "S")).toBeVisible();

      // Import the file: the shortcuts come back.
      await find(settings, "");
      await pickFiles(app, [exported]);
      await settings.getByRole("button", { name: "Import shortcuts…" }).click();
      await expect(settings.getByText("Shortcuts imported.")).toBeVisible();
      await find(settings, "split");
      await expect(keysOf(settings, SPLIT, `${MOD_NAME}+K, S`)).toBeVisible();
      await closeSettings(settings);

      // Relaunch on the same profile: the bindings persist, and Ctrl+K still splits.
      await first.close();
      const second = await launchFlowApp(work, home);
      try {
        const page2 = second.page;
        await clipWithPlayhead(page2, second.app, work, "Keys again");
        const again = await openSettingsPage(page2, second.app, "Keyboard shortcuts");
        await find(again, "split");
        await expect(keysOf(again, SPLIT, `${MOD_NAME}+K, S`)).toBeVisible();
        await find(again, "redo");
        await expect(keysOf(again, "Redo", `${MOD_NAME}+Shift+Z, ${MOD_NAME}+Y`)).toBeVisible();
        await snapshot(page2, info, "shortcuts after relaunch");
        await closeSettings(again);
        await selectAndPlace(page2);
        await page2.keyboard.press(`${MOD}+K`);
        await expect.poll(() => layout(page2)).toEqual(SPLIT_IN_TWO);
        await editorCaughtUp(page2);

        // Tidy up as a person would: everything back to the preset.
        const tidy = await openSettingsPage(page2, second.app, "Keyboard shortcuts");
        await tidy.getByRole("button", { name: "Reset all shortcuts" }).click();
        await page2
          .getByRole("dialog", { name: "Reset all shortcuts?" })
          .getByRole("button", { name: "Reset shortcuts" })
          .click();
        await expect(
          tidy.getByText("All shortcuts are back to the Cutweave preset."),
        ).toBeVisible();
        await closeSettings(tidy);
      } finally {
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });

  test("the CapCut preset splits with Ctrl+B; typing in the inspector's name field runs nothing", async () => {
    const work = workspace();
    const home = profileDir();
    try {
      const first = await launchFlowApp(work, home);
      const { app, page } = first;
      // Clips stay where they are dropped; the setting is kept for the relaunch below.
      await freeMainTrack(app, page);
      await clipWithPlayhead(page, app, work, "CapCut keys");

      // With the NLE preset Ctrl+B does nothing on the timeline.
      await page.keyboard.press(`${MOD}+B`);
      await editorCaughtUp(page);
      expect(await layout(page)).toEqual(WHOLE);

      // Typing in the inspector's name field is the field's: S, N and Ctrl+B change no clip.
      const name = inspector(page).name;
      await name.click();
      await page.keyboard.type("sn");
      await page.keyboard.press(`${MOD}+B`);
      await expect(name).toHaveValue(/sn/);
      expect(await layout(page)).toEqual(WHOLE);
      await page.keyboard.press("Escape");

      // Switch the preset to CapCut: Ctrl+B splits at the playhead, and S no longer does.
      const settings = await openSettingsPage(page, app, "Keyboard shortcuts");
      await settings
        .getByRole("radiogroup", { name: "Shortcut preset" })
        .getByRole("radio", { name: "CapCut" })
        .click();
      await find(settings, "split");
      await expect(keysOf(settings, SPLIT, `${MOD_NAME}+B`)).toBeVisible();
      await closeSettings(settings);
      await selectAndPlace(page);
      await page.keyboard.press("s");
      await editorCaughtUp(page);
      expect(await layout(page)).toEqual(WHOLE);
      await page.keyboard.press(`${MOD}+B`);
      await expect.poll(() => layout(page)).toEqual(SPLIT_IN_TWO);
      await editorCaughtUp(page);

      // The preset persists across a relaunch.
      await first.close();
      const second = await launchFlowApp(work, home);
      try {
        const again = await openSettingsPage(second.page, second.app, "Keyboard shortcuts");
        await expect(
          again
            .getByRole("radiogroup", { name: "Shortcut preset" })
            .getByRole("radio", { name: "CapCut" }),
        ).toHaveAttribute("aria-checked", "true");
        // Back to the Cutweave preset, as a person tidies up.
        await again
          .getByRole("radiogroup", { name: "Shortcut preset" })
          .getByRole("radio", { name: "Cutweave (NLE)" })
          .click();
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
