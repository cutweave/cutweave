// SPDX-License-Identifier: GPL-3.0-or-later
// F28 Playback preferences (docs/user-flows.md, WP-2H.10): Settings > Playback has "Go back to
// where playback started". Off, the default: Space plays and pauses, and the playhead stays where
// playback stopped. On: pausing puts the playhead back where Space was pressed, and so does
// playback that runs into the end of the timeline. After a relaunch the switch is still on and a
// new project behaves the same.
//
// The loop, preview quality, proxies and scrub audio steps of this flow join it with the work
// that gives the engine each behaviour (docs/reference/settings.md, Playback).
import type { ElectronApplication, Page } from "@playwright/test";
import { expect, test } from "../fixtures.ts";
import {
  closeSettings,
  createProject,
  editorCaughtUp,
  hasRealEngine,
  importWithButton,
  launchFlowApp,
  layout,
  mediaCard,
  openSettingsPage,
  profileDir,
  removeDir,
  rulerY,
  sec,
  snapshot,
  timecode,
  timelineGeometry,
  type Workspace,
  workspace,
  xAt,
} from "./helpers.ts";

const SWITCH = "Go back to where playback started";

/** `HH:MM:SS:FF` → frames at 30 fps (the new project's rate). */
function frames(text: string | null): number {
  const [hh, mm, ss, ff] = (text ?? "").split(/[:;]/).map(Number);
  return (((hh ?? 0) * 60 + (mm ?? 0)) * 60 + (ss ?? 0)) * 30 + (ff ?? 0);
}

const timecodeFrames = async (page: Page): Promise<number> =>
  frames(await timecode(page).textContent());

/** Keys go to the page (the keymap), not to a focused control. */
async function press(page: Page, key: string): Promise<void> {
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press(key);
}

/** A new project with a 5 s picture on the timeline, ready to play. */
async function projectWithClip(
  app: ElectronApplication,
  page: Page,
  name: string,
  work: Workspace,
) {
  await createProject(app, page, name, work.dir);
  await importWithButton(app, page, [work.file("logo.png")]);
  await mediaCard(page, "logo.png").click();
  await page.keyboard.press("Enter");
  await expect.poll(async () => (await layout(page))[0]?.clips.length ?? 0).toBe(1);
  await editorCaughtUp(page);
  await expect(page.getByRole("button", { name: "Play", exact: true })).toBeEnabled();
}

/** Clicks the ruler at `seconds`, and waits for the timecode to say so. */
async function goTo(page: Page, seconds: number): Promise<void> {
  const g = await timelineGeometry(page);
  await page.mouse.click(xAt(g, sec(seconds)), rulerY(g));
  await expect.poll(() => timecodeFrames(page)).toBe(seconds * 30);
}

/** Space, wait until playback is at least half a second on, Space again. */
async function playAndPause(page: Page, fromFrames: number): Promise<void> {
  await press(page, "Space");
  await expect(page.getByRole("button", { name: "Pause", exact: true })).toBeVisible();
  await expect
    .poll(() => timecodeFrames(page), { timeout: 20_000 })
    .toBeGreaterThan(fromFrames + 15);
  await press(page, "Space");
  await expect(page.getByRole("button", { name: "Play", exact: true })).toBeVisible();
}

test.describe("F28 playback preferences", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("go back to where playback started: off, on, at the end, and after a relaunch", async () => {
    test.setTimeout(240_000);
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    try {
      const first = await launchFlowApp(work, home);
      try {
        const { app, page } = first;
        await projectWithClip(app, page, "Playback", work);

        // Off (the default): a pause stays where playback stopped.
        await goTo(page, 1);
        await playAndPause(page, 30);
        await expect.poll(() => timecodeFrames(page)).toBeGreaterThan(30 + 15);

        // Settings > Playback: the page, and its switch, off.
        let settings = await openSettingsPage(page, app, "Playback");
        const toggle = settings.getByRole("switch", { name: SWITCH });
        await expect(toggle).not.toBeChecked();
        await toggle.click();
        await expect(toggle).toBeChecked();
        await snapshot(page, info, "settings playback");
        await closeSettings(settings);

        // On: the pause comes back to 1 s, where Space was pressed.
        await goTo(page, 1);
        await playAndPause(page, 30);
        await expect(timecode(page)).toHaveText("00:00:01:00");

        // Playing into the end of the timeline (5 s) comes back too.
        await goTo(page, 4);
        await press(page, "Space");
        await expect(page.getByRole("button", { name: "Pause", exact: true })).toBeVisible();
        await expect(page.getByRole("button", { name: "Play", exact: true })).toBeVisible({
          timeout: 20_000,
        });
        await expect(timecode(page)).toHaveText("00:00:04:00");

        // Found by search, from any page.
        settings = await openSettingsPage(page, app, "General");
        await settings.getByRole("searchbox", { name: "Search settings" }).fill("rewind");
        await expect(settings.getByRole("switch", { name: SWITCH })).toBeChecked();
        await closeSettings(settings);
      } finally {
        await first.close();
      }

      // Relaunch: the switch is still on, and a new project's playback returns.
      const second = await launchFlowApp(work, home);
      try {
        const { app, page } = second;
        const settings = await openSettingsPage(page, app, "Playback");
        await expect(settings.getByRole("switch", { name: SWITCH })).toBeChecked();
        await closeSettings(settings);
        await projectWithClip(app, page, "Again", work);
        await goTo(page, 2);
        await playAndPause(page, 60);
        await expect(timecode(page)).toHaveText("00:00:02:00");
      } finally {
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
