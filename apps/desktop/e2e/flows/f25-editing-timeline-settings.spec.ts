// SPDX-License-Identifier: GPL-3.0-or-later
// F25 Editing and timeline preferences (docs/user-flows.md, WP-2A.13): Settings > Timeline and
// Settings > Editing, each changed the way a person does and seen where it shows.
//
// - "Snapping on when a project opens" off, relaunch, open the project: the timeline's Snapping
//   button is off.
// - "Show time as" Frames: the time under the preview and the Inspector's Start and Duration
//   show frames (the ruler draws the same labels; what it draws is checked in Vitest).
// - "Height of new tracks" Large, then a card dropped where it does not fit: the track the drop
//   makes is large, and the tracks that were there keep their height.
// - "Undo steps" 2, then three edits: two undo and the third does not.
// - "Follow the playhead while playing" Off, play past the edge of the view: the view stays
//   where it was; back on Page, it turns the page. Playing needs an engine built with FFmpeg
//   and the fetched corpus, like F13; without them that test skips and says why.
import type { ElectronApplication, Locator, Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import { copyCorpusClips, corpusDir } from "./export-helpers.ts";
import {
  clickClip,
  clipAt,
  closeSettings,
  createProject,
  dirtyDot,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  importWithButton,
  inspector,
  launchFlowApp,
  layout,
  mediaCard,
  mediaCards,
  movePlayhead,
  openRecent,
  openSettings,
  pickFiles,
  profileDir,
  removeDir,
  sec,
  sequence,
  shortcut,
  showTimeline,
  snapshot,
  timecode,
  timelineGeometry,
  timelineTools,
  toolbar,
  viewSettled,
  workspace,
} from "./helpers.ts";

/** File > Settings…, then the Editing or the Timeline page. */
async function settingsPage(
  page: Page,
  app: ElectronApplication,
  name: "Editing" | "Timeline",
): Promise<Locator> {
  const settings = await openSettings(page, app);
  await settings.getByRole("tab", { name }).click();
  await expect(settings.getByRole("heading", { name, level: 3 })).toBeVisible();
  return settings;
}

/** Picks one choice of a row of Settings > Timeline and closes the dialog. */
async function chooseTimeline(
  page: Page,
  app: ElectronApplication,
  row: string,
  choice: string,
): Promise<void> {
  const settings = await settingsPage(page, app, "Timeline");
  const radio = settings.getByRole("radiogroup", { name: row }).getByRole("radio", { name: choice });
  await radio.click();
  await expect(radio).toHaveAttribute("aria-checked", "true");
  await closeSettings(settings);
}

const snappingButton = (page: Page): Locator =>
  timelineTools(page).getByRole("button", { name: "Snapping", exact: true });

/** How many clips the sequence holds. */
const clipCount = async (page: Page): Promise<number> =>
  (await layout(page)).reduce((n, track) => n + track.clips.length, 0);

/** The sequence's tracks with their heights (`null` is the default height). */
async function trackHeights(page: Page): Promise<{ name: string; height: number | null }[]> {
  const seq = await sequence(page);
  const out = await editorCall<{ tracks: { name: string; height: number | null }[] }>(
    page,
    "track.list",
    { sequence: seq.id },
  );
  return out.tracks.map((t) => ({ name: t.name, height: t.height ?? null }));
}

/** Selects the card and presses Enter: the clip goes in at the playhead. */
async function insertCard(page: Page, name: string): Promise<void> {
  const before = await clipCount(page);
  await mediaCard(page, name).click();
  await page.keyboard.press("Enter");
  await expect.poll(() => clipCount(page)).toBeGreaterThan(before);
  await editorCaughtUp(page);
}

test.describe("F25 editing and timeline preferences", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("snapping at open, time as frames, large new tracks and two undo steps", async () => {
    test.setTimeout(300_000);
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    try {
      // ---- "Snapping on when a project opens": off, and a project to open after a relaunch.
      const first = await launchFlowApp(work, home);
      try {
        const { app, page } = first;
        await createProject(app, page, "Prefs", work.dir);
        await importWithButton(app, page, [work.file("interview.mp4")]);
        await expect(snappingButton(page)).toHaveAttribute("aria-pressed", "true");
        const settings = await settingsPage(page, app, "Timeline");
        const snap = settings.getByRole("switch", { name: "Snapping on when a project opens" });
        await expect(snap).toHaveAttribute("aria-checked", "true");
        await snap.click();
        await expect(snap).toHaveAttribute("aria-checked", "false");
        await snapshot(page, info, "settings-timeline");
        await closeSettings(settings);
        // The button is the session's: the setting is for the next project that opens.
        await expect(snappingButton(page)).toHaveAttribute("aria-pressed", "true");
        await shortcut(app, "Control+S");
        await expect(dirtyDot(page)).toBeHidden();
      } finally {
        await first.close();
      }

      const second = await launchFlowApp(work, home);
      try {
        const { app, page } = second;
        await openRecent(page, "Prefs.cutweave");
        await expect(page).toHaveTitle("Prefs — Cutweave");
        await expect(mediaCards(page)).toHaveCount(1);
        await expect(snappingButton(page)).toHaveAttribute("aria-pressed", "false");
        // It is still a button: one click turns snapping on for this session.
        await snappingButton(page).click();
        await expect(snappingButton(page)).toHaveAttribute("aria-pressed", "true");

        // ---- "Show time as" Frames: the transport and the Inspector.
        await insertCard(page, "interview.mp4"); // 0 s .. 10 s, with its sound
        await showTimeline(page, 26);
        await viewSettled(page);
        await movePlayhead(page, 12);
        await expect(timecode(page)).toHaveText("00:00:12:00");
        await chooseTimeline(page, app, "Show time as", "Frames");
        await expect(timecode(page)).toHaveText("360"); // 12 s at 30 fps
        const g = await timelineGeometry(page);
        await clickClip(page, clipAt(g, "V1", 0).id);
        await expect(inspector(page).start).toHaveValue("f:0");
        await expect(inspector(page).duration).toHaveValue("f:300");
        await snapshot(page, info, "time-as-frames");
        // A frame typed into the field is read as a frame: the clip is 8 s long now.
        await inspector(page).duration.fill("f:240");
        await inspector(page).duration.press("Enter");
        await expect.poll(async () => (await layout(page))[0]?.clips[0]).toEqual([0, 8]);
        await chooseTimeline(page, app, "Show time as", "Seconds");
        await expect(timecode(page)).toHaveText("12s");
        await expect(inspector(page).duration).toHaveValue("8s");
        await chooseTimeline(page, app, "Show time as", "Timecode");
        await expect(timecode(page)).toHaveText("00:00:12:00");

        // ---- "Height of new tracks" Large: a card dropped where it does not fit makes a track.
        await insertCard(page, "interview.mp4"); // at the playhead: 12 s .. 22 s
        await expect
          .poll(async () => (await layout(page)).find((t) => t.track === "V1")?.clips)
          .toEqual([
            [0, 8],
            [12, 10],
          ]);
        expect((await trackHeights(page)).every((t) => t.height === null)).toBe(true);
        await chooseTimeline(page, app, "Height of new tracks", "Large");
        const large = await page.evaluate(() =>
          Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--track-lg")),
        );
        expect(large).toBeGreaterThan(0);
        const tracksBefore = (await trackHeights(page)).length;
        // 4 s of room between the clips, 10 s of clip: it lands on a new track.
        await dropCard(page, "interview.mp4", "V1", 9);
        await expect.poll(async () => (await trackHeights(page)).length).toBeGreaterThan(tracksBefore);
        await editorCaughtUp(page);
        await expect
          .poll(async () =>
            (await trackHeights(page))
              .filter((t) => t.name !== "V1" && t.name !== "A1")
              .map((t) => t.height),
          )
          .toContain(large);
        const heights = await trackHeights(page);
        // Every track the drop made is large; the ones that were there keep the default.
        for (const track of heights) {
          expect(track.height, track.name).toBe(
            track.name === "V1" || track.name === "A1" ? null : large,
          );
        }
        await snapshot(page, info, "large-new-track");
        // One undo takes the clip and its track away together.
        await shortcut(app, "Control+Z");
        await expect.poll(async () => (await trackHeights(page)).length).toBe(tracksBefore);
        await chooseTimeline(page, app, "Height of new tracks", "Medium");

        // ---- "Undo steps" 2: three edits, two undo, the third does not.
        const editing = await settingsPage(page, app, "Editing");
        const steps = editing.getByRole("spinbutton", { name: "Undo steps" });
        await expect(steps).toHaveValue("1000");
        await steps.fill("2");
        await expect(steps).toHaveValue("2");
        await closeSettings(editing);
        await expect
          .poll(async () => (await editorCall<{ history_limit: number }>(page, "project.info")).history_limit)
          .toBe(2);
        const start = await clipCount(page);
        const counts: number[] = [];
        for (let edit = 0; edit < 3; edit += 1) {
          await insertCard(page, "interview.mp4");
          counts.push(await clipCount(page));
        }
        const undo = toolbar(page).getByRole("button", { name: /^Undo/ });
        await expect(undo).toBeEnabled();
        await shortcut(app, "Control+Z");
        await expect.poll(() => clipCount(page)).toBe(counts[1]);
        await shortcut(app, "Control+Z");
        await expect.poll(() => clipCount(page)).toBe(counts[0]);
        // The first of the three edits is past the two steps kept: nothing left to undo.
        await expect(undo).toBeDisabled();
        await shortcut(app, "Control+Z");
        await editorCaughtUp(page);
        expect(await clipCount(page)).toBe(counts[0]);
        expect(counts[0]).toBeGreaterThan(start);
        // Redo still brings the two back.
        await shortcut(app, "Control+Y");
        await expect.poll(() => clipCount(page)).toBe(counts[1]);
      } finally {
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });

  test("follow Off keeps the view where it was while playing; Page turns it", async () => {
    test.setTimeout(300_000);
    const info = test.info();
    const corpus = corpusDir();
    test.skip(corpus === null, "the corpus is not fetched (just corpus)");
    const CLIP = "sintel_trailer_480p.mp4";
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work, undefined, { realMedia: true });
    try {
      const caps = await editorCall<{ available?: boolean }>(page, "media.capabilities");
      test.skip(
        caps.available !== true,
        "this engine has no media backend; build it with --features cutweave-engine/ffmpeg",
      );
      await createProject(app, page, "Follow", work.dir);
      await pickFiles(app, copyCorpusClips(corpus ?? "", work.media, [CLIP]));
      await page.getByRole("button", { name: "Import", exact: true }).click();
      await expect(mediaCards(page)).toHaveCount(1);
      await dropCard(page, CLIP, "V1", 0);
      await expect.poll(async () => (await layout(page))[0]?.clips.length ?? 0).toBe(1);
      await editorCaughtUp(page);
      const play = page.getByRole("button", { name: "Play", exact: true });
      const pause = page.getByRole("button", { name: "Pause", exact: true });
      await expect(play).toBeEnabled({ timeout: 30_000 });

      // Zoom in until the lanes show about two seconds, so playing soon passes the edge.
      const zoomIn = timelineTools(page).getByRole("button", { name: "Zoom in", exact: true });
      let g = await timelineGeometry(page);
      const lane = g.box.width - g.headerWidth;
      for (let n = 0; n < 16 && lane / g.pxPerSec > 2; n += 1) {
        const was = g.pxPerSec;
        await zoomIn.click();
        await expect.poll(async () => (await timelineGeometry(page)).pxPerSec).toBeGreaterThan(was);
        g = await timelineGeometry(page);
      }
      expect(lane / g.pxPerSec).toBeLessThanOrEqual(2);
      await viewSettled(page);
      const scrollX = async (): Promise<number> =>
        (await editorCall<{ scroll: { x: number } }>(page, "view.get")).scroll.x;
      /** The time under the preview (`HH:MM:SS:FF` at the project's 30 fps), in flicks. */
      const playhead = async (): Promise<number> => {
        const [hh, mm, ss, ff] = ((await timecode(page).textContent()) ?? "")
          .split(/[:;]/)
          .map(Number);
        const frames = (((hh ?? 0) * 60 + (mm ?? 0)) * 60 + (ss ?? 0)) * 30 + (ff ?? 0);
        return sec(frames / 30);
      };
      const before = await scrollX();
      const edge = before + sec(lane / g.pxPerSec);

      // Off: the playhead runs past the right edge and the view does not move.
      await chooseTimeline(page, app, "Follow the playhead while playing", "Off");
      await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
      await page.keyboard.press("Space");
      await expect(pause).toBeVisible();
      await expect.poll(playhead, { timeout: 30_000 }).toBeGreaterThan(edge + sec(0.5));
      await page.keyboard.press("Space");
      await expect(play).toBeVisible();
      await viewSettled(page);
      expect(await scrollX()).toBe(before);
      await snapshot(page, info, "follow-off");

      // Page: playing on, the view turns to where the playhead is.
      await chooseTimeline(page, app, "Follow the playhead while playing", "Page");
      await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
      const from = await playhead();
      await page.keyboard.press("Space");
      await expect(pause).toBeVisible();
      await expect.poll(playhead, { timeout: 30_000 }).toBeGreaterThan(from + sec(0.5));
      await page.keyboard.press("Space");
      await expect(play).toBeVisible();
      await viewSettled(page);
      const after = await scrollX();
      expect(after).toBeGreaterThan(before);
      // The playhead is in sight again.
      const at = await playhead();
      expect(at).toBeGreaterThanOrEqual(after);
      expect(at).toBeLessThanOrEqual(after + sec(lane / g.pxPerSec));
      await snapshot(page, info, "follow-page");
    } finally {
      await close();
      work.remove();
    }
  });
});
