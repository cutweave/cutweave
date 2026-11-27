// SPDX-License-Identifier: GPL-3.0-or-later
// F28 Playback preferences (docs/user-flows.md, WP-2H.10): Settings > Playback.
//
// - "Go back to where playback started". Off, the default: Space plays and pauses, and the
//   playhead stays where playback stopped. On: pausing puts the playhead back where Space was
//   pressed, and so does playback that runs into the end of the timeline.
// - "Loop playback": playback passes the end and plays on from the start; the transport bar's
//   loop button is the same value, and turns it off again.
// - "Preview quality while playing": Half in Settings, then Quarter from the menu of the
//   transport's quality indicator; the indicator says so while playing, and Full when paused.
// - "Use proxies in the preview": the transport bar's toggle follows the setting.
// - After a relaunch every value is as it was left, and a new project plays with them.
//
// Playback needs the full engine: built with FFmpeg (`just e2e-flows` does when the bundle is set
// up) and the fetched corpus (`just corpus`); without either the flow skips and says why.
import type { ElectronApplication, Locator, Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import { copyCorpusClips, corpusDir } from "./export-helpers.ts";
import {
  closeSettings,
  createProject,
  editorCaughtUp,
  hasRealEngine,
  launchFlowApp,
  layout,
  mediaCard,
  mediaCards,
  openSettingsPage,
  pickFiles,
  profileDir,
  removeDir,
  revealTime,
  rulerY,
  sec,
  snapshot,
  timecode,
  type Workspace,
  workspace,
  xAt,
} from "./helpers.ts";

const CLIP = "sintel_trailer_480p.mp4";
const SWITCH = "Go back to where playback started";
const LOOP = "Loop playback";
const PROXIES = "Use proxies in the preview";

const playButton = (page: Page) => page.getByRole("button", { name: "Play", exact: true });
const pauseButton = (page: Page) => page.getByRole("button", { name: "Pause", exact: true });
/** The transport bar's loop button and proxy toggle (the settings rows are switches). */
const loopButton = (page: Page) => page.getByRole("button", { name: LOOP, exact: true });
const proxyButton = (page: Page) => page.getByRole("button", { name: PROXIES, exact: true });
/** The transport bar's quality indicator, by what it says. */
const quality = (page: Page, name: "Full" | "Half" | "Quarter") =>
  page.getByLabel(`Preview quality: ${name}`, { exact: true });
/** Settings > Playback > Preview quality while playing. */
const playingQuality = (settings: Locator) =>
  settings.getByRole("radiogroup", { name: "Preview quality while playing" });

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

/** A new project with the corpus clip on the timeline, ready to play. */
async function projectWithClip(
  app: ElectronApplication,
  page: Page,
  name: string,
  work: Workspace,
  corpus: string,
) {
  await createProject(app, page, name, work.dir);
  await pickFiles(app, copyCorpusClips(corpus, work.media, [CLIP]));
  await page.getByRole("button", { name: "Import", exact: true }).click();
  await expect(mediaCards(page)).toHaveCount(1);
  await mediaCard(page, CLIP).click();
  await page.keyboard.press("Enter");
  await expect.poll(async () => (await layout(page))[0]?.clips.length ?? 0).toBe(1);
  await editorCaughtUp(page);
  await expect(page.getByRole("button", { name: "Play", exact: true })).toBeEnabled({
    timeout: 30_000,
  });
}

/** Clicks the ruler at `seconds`, and waits for the timecode to say so. */
async function goTo(page: Page, seconds: number): Promise<void> {
  // The timeline follows the playhead while playing (WP-2A.13), so the place may be out of view.
  const g = await revealTime(page, seconds);
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

  test("go back to the start, loop, the quality while playing, proxies, and a relaunch", async () => {
    test.setTimeout(360_000);
    const info = test.info();
    const corpus = corpusDir();
    test.skip(corpus === null, "the corpus is not fetched (just corpus)");
    const work = workspace();
    const home = profileDir();
    try {
      const first = await launchFlowApp(work, home, { realMedia: true });
      try {
        const { app, page } = first;
        const caps = await editorCall<{ available?: boolean }>(page, "media.capabilities");
        test.skip(
          caps.available !== true,
          "this engine has no media backend; build it with --features cutweave-engine/ffmpeg",
        );
        await projectWithClip(app, page, "Playback", work, corpus ?? "");

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

        // Playing into the end of the timeline comes back too: from 45 frames before the end.
        await page.getByRole("button", { name: "Go to end" }).click();
        const last = Math.round(((await layout(page))[0]?.clips[0]?.[1] ?? 0) * 30) - 1;
        await expect.poll(() => timecodeFrames(page)).toBe(last);
        for (let i = 1; i <= 45; i += 1) {
          await press(page, "ArrowLeft");
          await expect.poll(() => timecodeFrames(page)).toBe(last - i);
        }
        await press(page, "Space");
        await expect(page.getByRole("button", { name: "Pause", exact: true })).toBeVisible();
        await expect(page.getByRole("button", { name: "Play", exact: true })).toBeVisible({
          timeout: 20_000,
        });
        await expect.poll(() => timecodeFrames(page)).toBe(last - 45);

        // Found by search, from any page.
        settings = await openSettingsPage(page, app, "General");
        await settings.getByRole("searchbox", { name: "Search settings" }).fill("rewind");
        await expect(settings.getByRole("switch", { name: SWITCH })).toBeChecked();
        await closeSettings(settings);

        // Loop playback, turned on in Settings: the transport's loop button shows it.
        await expect(loopButton(page)).toHaveAttribute("aria-pressed", "false");
        settings = await openSettingsPage(page, app, "Playback");
        await settings.getByRole("switch", { name: LOOP }).click();
        await expect(settings.getByRole("switch", { name: LOOP })).toBeChecked();
        await closeSettings(settings);
        await expect(loopButton(page)).toHaveAttribute("aria-pressed", "true");
        // From 45 frames before the end, playback passes the end and plays on from the start.
        await expect.poll(() => timecodeFrames(page)).toBe(last - 45);
        await press(page, "Space");
        await expect(pauseButton(page)).toBeVisible();
        await expect.poll(() => timecodeFrames(page), { timeout: 30_000 }).toBeLessThan(last - 100);
        await expect(pauseButton(page)).toBeVisible();
        await expect.poll(() => timecodeFrames(page), { timeout: 20_000 }).toBeGreaterThan(5);
        // Pausing still goes back to where Space was pressed.
        await press(page, "Space");
        await expect(playButton(page)).toBeVisible();
        await expect.poll(() => timecodeFrames(page)).toBe(last - 45);
        // The transport's button turns the same setting off: playback stops at the end again.
        await loopButton(page).click();
        await expect(loopButton(page)).toHaveAttribute("aria-pressed", "false");
        settings = await openSettingsPage(page, app, "Playback");
        await expect(settings.getByRole("switch", { name: LOOP })).not.toBeChecked();

        // Preview quality while playing: Auto by default; Half.
        await expect(playingQuality(settings).getByRole("radio", { name: "Auto" })).toBeChecked();
        await playingQuality(settings).getByRole("radio", { name: "Half" }).click();
        await expect(playingQuality(settings).getByRole("radio", { name: "Half" })).toBeChecked();
        await closeSettings(settings);
        await goTo(page, 1);
        await expect(quality(page, "Full")).toBeVisible();
        await press(page, "Space");
        await expect(pauseButton(page)).toBeVisible();
        await expect(quality(page, "Half")).toBeVisible({ timeout: 20_000 });
        await expect.poll(() => timecodeFrames(page), { timeout: 20_000 }).toBeGreaterThan(45);
        await expect(quality(page, "Half")).toBeVisible();
        await snapshot(page, info, "playing at half");
        // Paused, the frame refines to full size (the default when paused).
        await press(page, "Space");
        await expect(playButton(page)).toBeVisible();
        await expect(quality(page, "Full")).toBeVisible({ timeout: 20_000 });

        // The indicator's own menu chooses the same value: Quarter.
        await quality(page, "Full").click({ button: "right" });
        await expect(page.getByRole("menuitemradio", { name: "Half" })).toBeChecked();
        await page.getByRole("menuitemradio", { name: "Quarter" }).click();
        await press(page, "Space");
        await expect(pauseButton(page)).toBeVisible();
        await expect(quality(page, "Quarter")).toBeVisible({ timeout: 20_000 });
        await press(page, "Space");
        await expect(playButton(page)).toBeVisible();
        await expect(proxyButton(page)).toHaveAttribute("aria-pressed", "true");
        settings = await openSettingsPage(page, app, "Playback");
        await expect(
          playingQuality(settings).getByRole("radio", { name: "Quarter" }),
        ).toBeChecked();

        // Proxies: the setting and the transport bar's toggle are one value.
        await settings.getByRole("switch", { name: PROXIES }).click();
        await expect(settings.getByRole("switch", { name: PROXIES })).not.toBeChecked();
        await closeSettings(settings);
        await expect(proxyButton(page)).toHaveAttribute("aria-pressed", "false");
      } finally {
        await first.close();
      }

      // Relaunch: every value is as it was left, and a new project plays with them.
      const second = await launchFlowApp(work, home, { realMedia: true });
      try {
        const { app, page } = second;
        const settings = await openSettingsPage(page, app, "Playback");
        await expect(settings.getByRole("switch", { name: SWITCH })).toBeChecked();
        await expect(settings.getByRole("switch", { name: LOOP })).not.toBeChecked();
        await expect(settings.getByRole("switch", { name: PROXIES })).not.toBeChecked();
        await expect(
          playingQuality(settings).getByRole("radio", { name: "Quarter" }),
        ).toBeChecked();
        await closeSettings(settings);
        await expect(loopButton(page)).toHaveAttribute("aria-pressed", "false");
        await expect(proxyButton(page)).toHaveAttribute("aria-pressed", "false");
        await projectWithClip(app, page, "Again", work, corpus ?? "");
        await goTo(page, 2);
        await press(page, "Space");
        await expect(pauseButton(page)).toBeVisible();
        // The engine of this launch was told the quality again.
        await expect(quality(page, "Quarter")).toBeVisible({ timeout: 20_000 });
        await expect.poll(() => timecodeFrames(page), { timeout: 20_000 }).toBeGreaterThan(75);
        await press(page, "Space");
        await expect(playButton(page)).toBeVisible();
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
