// SPDX-License-Identifier: GPL-3.0-or-later
// M1 exit criteria 1 and 2, the UI half (WP-1O.04, docs/milestones/M1-exit.md).
//
// Criterion 1: through the UI, import three clips, arrange, trim, split, export a 1080p MP4 that
// plays. The clips are the M1 suite's corpus clips: H.264 1080p60, HEVC 10-bit and a VFR phone
// clip rotated 90°. Like every flow it drives the app as a person does (ADR-0018): F14 does the
// same with pointer drags; this cut uses exact inputs so an agent can make the same one
// (criterion 2, `crates/cutweave-cli/tests/mcp_conformance/m1.rs`):
//
// - Enter in the media bin inserts the selected clip at the playhead;
// - the inspector's Duration and Start fields take typed times (`clip.trim`, `item.move`);
// - S splits the selected clip at the playhead;
// - the playhead goes on a frame with a click on the ruler (checked on the timecode), or to the
//   end of the timeline with a click on the ruler past it (the engine stops it there).
//
// Under the gate (`CUTWEAVE_M1_OUT`) the project folder with its export is kept for the
// comparison with the agent's (`scripts/m1-gate.py`). Only the OS pickers are stubbed.
import { readFileSync, statSync } from "node:fs";
import path from "node:path";
import type { Page } from "@playwright/test";
import { editorCall, expect, test } from "../fixtures.ts";
import {
  copyCorpusClips,
  corpusDir,
  ffprobePath,
  playInChromium,
  probe,
} from "../flows/export-helpers.ts";
import {
  clickClip,
  clipAt,
  createProject,
  dirtyDot,
  editorCaughtUp,
  hasRealEngine,
  importWithButton,
  inspector,
  launchFlowApp,
  layout,
  mediaCard,
  rulerY,
  sec,
  shortcut,
  snapshot,
  timecode,
  timelineCanvas,
  timelineGeometry,
  toast,
  workspace,
  xAt,
} from "../flows/helpers.ts";
import { keepCopy, keepJson, M1_CLIPS, M1_PROJECT, need } from "./m1-support.ts";

/** The timeline's length after the cut, in seconds (30 fps). */
const LENGTH = 12;

/** `00:00:SS:00` for whole seconds below a minute. */
const tc = (s: number): string => `00:00:${String(s).padStart(2, "0")}:00`;

/** Clips per track as [start, duration] in seconds. */
const tracks = async (page: Page) => {
  const all = await layout(page);
  return {
    v1: all.find((t) => t.track === "V1")?.clips ?? [],
    a1: all.find((t) => t.track === "A1")?.clips ?? [],
  };
};

/**
 * Puts the playhead at `at` seconds: on a frame inside the timeline with a click on the ruler, or
 * at the end with a click on the ruler past it (the engine stops the playhead at the end).
 */
async function playheadTo(page: Page, at: number, pastTheEnd: boolean): Promise<void> {
  const g = await timelineGeometry(page);
  await page.mouse.click(xAt(g, sec(pastTheEnd ? at + 1.5 : at)), rulerY(g));
  await expect(timecode(page)).toHaveText(tc(at));
}

/**
 * Places a bin clip at the playhead (select its card, Enter) and types its length into the
 * inspector's Duration field (the new clip is selected). Resolves when V1 shows `v1`.
 */
async function place(
  page: Page,
  name: string,
  duration: number,
  v1: [number, number][],
): Promise<void> {
  const before = (await tracks(page)).v1.length;
  await mediaCard(page, name).click();
  await page.keyboard.press("Enter");
  await expect.poll(async () => (await tracks(page)).v1.length).toBe(before + 1);
  await editorCaughtUp(page);
  await expect(page.getByTestId("clip-inspector")).toBeVisible();
  await inspector(page).duration.fill(`${duration}s`);
  await page.keyboard.press("Enter");
  await expect.poll(async () => (await tracks(page)).v1).toEqual(v1);
  await editorCaughtUp(page);
}

test.describe("M1 exit 1 and 2 (UI)", () => {
  test("import the three M1 clips, cut them with exact inputs, export a 1080p MP4 that plays", async () => {
    test.setTimeout(420_000);
    need(hasRealEngine, "cutweave-engine-serve is not built");
    const info = test.info();
    const corpus = corpusDir();
    need(corpus !== null, "the corpus is not fetched (just corpus)");
    const ffprobe = ffprobePath();
    need(ffprobe !== null, "ffprobe is not on this machine (set FFMPEG_DIR)");
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work, undefined, { realMedia: true });
    try {
      const caps = await editorCall<{ available?: boolean }>(page, "media.capabilities");
      need(
        caps.available === true,
        "this engine has no media backend; build it with --features cutweave-engine/ffmpeg",
      );

      // A project, the three clips imported with the bin's Import button.
      const doc = await createProject(app, page, M1_PROJECT, work.dir);
      const projectDir = path.dirname(doc);
      const files = copyCorpusClips(corpus ?? "", work.media, M1_CLIPS);
      await importWithButton(app, page, files);
      const refused = page.getByText(/not imported/);
      expect(await refused.count()).toBe(0);

      // Zoom out (the - key, twice) so the whole cut fits on the timeline.
      await timelineCanvas(page).locator("canvas").focus();
      const zoom = async () =>
        (await editorCall<{ zoom: number | null }>(page, "view.get")).zoom ?? 100;
      await page.keyboard.press("-");
      await expect.poll(zoom).toBe(80);
      await page.keyboard.press("-");
      await expect.poll(zoom).toBeCloseTo(64, 6);

      // Arrange and trim: A at 0-4 s, B at 4-7 s, C at 7-10 s, back to back on V1.
      await expect(timecode(page)).toHaveText(tc(0));
      await place(page, "h264_1080p60.mp4", 4, [[0, 4]]);
      await playheadTo(page, 4, true);
      await place(page, "hevc_10bit_sdr.mp4", 3, [
        [0, 4],
        [4, 3],
      ]);
      await playheadTo(page, 7, true);
      await place(page, "vfr_phone_rotated90.mp4", 3, [
        [0, 4],
        [4, 3],
        [7, 3],
      ]);
      // The HEVC clip has no sound; the other two brought theirs to A1.
      expect((await tracks(page)).a1).toEqual([
        [0, 4],
        [7, 3],
      ]);

      // Split A at 2 s: the playhead on 2 s, select A, S.
      await playheadTo(page, 2, false);
      let g = await timelineGeometry(page);
      await clickClip(page, clipAt(g, "V1", 0).id);
      await page.keyboard.press("s");
      await expect
        .poll(async () => (await tracks(page)).v1)
        .toEqual([
          [0, 2],
          [2, 2],
          [4, 3],
          [7, 3],
        ]);
      await editorCaughtUp(page);

      // Move A's right half to the end: select it, type 10 s into Start.
      g = await timelineGeometry(page);
      await clickClip(page, clipAt(g, "V1", 2).id);
      await inspector(page).start.fill("10s");
      await page.keyboard.press("Enter");
      await expect
        .poll(() => tracks(page))
        .toEqual({
          v1: [
            [0, 2],
            [4, 3],
            [7, 3],
            [10, 2],
          ],
          a1: [
            [0, 2],
            [7, 3],
            [10, 2],
          ],
        });
      await editorCaughtUp(page);
      await snapshot(page, info, "the M1 cut");

      // Save (Ctrl+S): the document on disk has the seven clips.
      await shortcut(app, "Control+S");
      await expect(dirtyDot(page)).toBeHidden();
      const saved = JSON.parse(readFileSync(doc, "utf8")) as {
        sequences: Record<string, { tracks: Record<string, { items: Record<string, unknown> }> }>;
      };
      const onDisk = Object.values(saved.sequences).flatMap((s) =>
        Object.values(s.tracks).flatMap((t) => Object.keys(t.items)),
      );
      expect(onDisk).toHaveLength(7);

      // Export with the top bar's button: the proposed file in the project folder, the 1080p
      // preset's quality (High) and the software encoder, as the agent asks for.
      const exportButton = page.getByRole("button", { name: "Export", exact: true });
      const dialog = page.getByRole("dialog", { name: "Export video" });
      await exportButton.click();
      await expect(dialog).toBeVisible();
      const file = path.join(projectDir, `${M1_PROJECT}.mp4`);
      const norm = (p: string) => p.replace(/\\/g, "/").toLowerCase();
      expect(norm(await dialog.getByLabel("Save to").inputValue())).toBe(norm(file));
      await dialog.getByRole("button", { name: "Advanced" }).click();
      await expect(dialog.getByLabel("Width")).toHaveValue("1920");
      await expect(dialog.getByLabel("Height")).toHaveValue("1080");
      await expect(dialog.getByLabel("Frame rate")).toHaveValue("30");
      await dialog.getByRole("radio", { name: "High" }).click();
      await expect(dialog.getByRole("radio", { name: "High" })).toBeChecked();
      const hardware = dialog.getByRole("switch", { name: "Use hardware encoder" });
      if (await hardware.isChecked()) await hardware.click();
      await expect(hardware).not.toBeChecked();
      await snapshot(page, info, "export settings");
      await dialog.getByRole("button", { name: "Export", exact: true }).click();
      const done = toast(page, "Export complete");
      await expect(done).toBeVisible({ timeout: 300_000 });
      await expect(done).toContainText(`${M1_PROJECT}.mp4`);

      // The file: H.264 1920x1080 at 30 fps with AAC sound, the timeline's length within a
      // frame; Chromium plays it.
      expect(statSync(file).size).toBeGreaterThan(10_000);
      const probed = probe(ffprobe ?? "ffprobe", file);
      expect(probed.formatName).toContain("mp4");
      expect(probed.video).toMatchObject({ codec: "h264", width: 1920, height: 1080 });
      expect(probed.video?.frameRate).toBeCloseTo(30, 3);
      expect(probed.audio?.codec).toBe("aac");
      expect(Math.abs(probed.duration - LENGTH)).toBeLessThanOrEqual(1 / 30);
      const played = await playInChromium(file);
      expect(played.error).toBeNull();
      expect(played.readyState).toBeGreaterThanOrEqual(3);
      expect(played.videoWidth).toBe(1920);
      expect(played.videoHeight).toBe(1080);
      expect(Math.abs(played.duration - LENGTH)).toBeLessThanOrEqual(1 / 30);
      info.annotations.push({ type: "ffprobe", description: JSON.stringify(probed) });
      info.annotations.push({ type: "video", description: JSON.stringify(played) });

      // Exporting did not touch the timeline.
      expect((await tracks(page)).v1).toHaveLength(4);
      const ui = { layout: await layout(page), ffprobe: probed, chromium: played };

      // Quit (nothing unsaved), then keep the project folder, its media (next to it, where the
      // document's relative paths point, so the gate can render the project) and what the test
      // saw.
      await close();
      keepCopy(projectDir, path.join("ui", M1_PROJECT));
      for (const f of files) keepCopy(f, path.join("ui", "media", path.basename(f)));
      keepJson(path.join("ui", "session.json"), {
        project: `${M1_PROJECT}/${M1_PROJECT}.cutweave`,
        export: `${M1_PROJECT}/${M1_PROJECT}.mp4`,
        ...ui,
      });
    } finally {
      await close();
      work.remove();
    }
  });
});
