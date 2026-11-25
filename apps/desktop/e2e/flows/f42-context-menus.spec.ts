// SPDX-License-Identifier: GPL-3.0-or-later
// F42 Context menus (docs/user-flows.md, WP-2H.12): right-click a text field in the Inspector and
// choose Paste; press Shift+F10 and then the Menu key on the focused field and see the menu open
// at it, with Escape returning the focus; right-click dead space and see nothing. Real right
// clicks and keys; the clipboard is the OS's, filled the way a copy in another app would.
//
// WP-2H.13 adds the second flow: the menus of today's surfaces. Right-click a bin card (Insert at
// playhead), the empty bin (Import…), a timeline clip (it becomes the selection; Split at
// playhead, Ctrl+Z, Disable clip), a track header (Mute, Lock, then a drag on it is refused), an
// empty lane (Add track above), the ruler (Move playhead here), a dock tab (Reset layout), the
// save status (Save now); press the Menu key on a bin card and choose Remove from project, then
// cancel its dialog. A third flow right-clicks the Export chip and cancels the export (it needs
// the FFmpeg engine and the corpus, like F14, and skips without them). Every choice is checked as
// an undo step or as state the person sees. Only the OS file picker is stubbed.
import { expect, test } from "../fixtures.ts";
import { copyCorpusClips, corpusDir } from "./export-helpers.ts";
import {
  clickClip,
  clipAt,
  clipBox,
  createProject,
  dirtyDot,
  drag,
  dropCard,
  editorCaughtUp,
  hasRealEngine,
  importWithButton,
  inspector,
  laneY,
  launchFlowApp,
  layout,
  mediaCard,
  mediaCards,
  mediaNames,
  pickFiles,
  rulerY,
  saveStatus,
  sec,
  sequence,
  shortcut,
  snapshot,
  timecode,
  timelineGeometry,
  timelineTools,
  toast,
  workspace,
  xAt,
} from "./helpers.ts";

test.describe("F42 context menus", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");

  test("a text field has Paste, the keyboard opens a menu at the focused control, dead space has none", async () => {
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Menus", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
      await editorCaughtUp(page);
      await clickClip(page, clipAt(await timelineGeometry(page), "V1", 1).id);
      const { name } = inspector(page);
      const menu = page.getByRole("menu");

      // Dead space: a right-click on the empty end of the timeline's tool strip shows nothing.
      const strip = await timelineTools(page).boundingBox();
      if (strip === null) throw new Error("no timeline tool strip");
      await page.mouse.click(strip.x + strip.width - 6, strip.y + strip.height / 2, {
        button: "right",
      });
      await expect(menu).toHaveCount(0);

      // A text field: right-click offers the text actions; Paste puts the clipboard in the field.
      await app.evaluate(({ clipboard }, text) => clipboard.writeText(text), "Pasted title");
      await name.click();
      await page.keyboard.press("Control+A");
      await name.click({ button: "right" });
      const fieldMenu = page.getByRole("menu", { name: "Text actions for Name" });
      await expect(fieldMenu).toBeVisible();
      // Each row reads its name, then (when it cannot run) why, then its key.
      await expect(fieldMenu.getByRole("menuitem")).toHaveText([
        /^Undo/,
        /^Cut/,
        /^Copy/,
        /^Paste/,
        /^Select all/,
      ]);
      await snapshot(page, info, "text field menu");
      await fieldMenu.getByRole("menuitem", { name: "Paste" }).click();
      await expect(menu).toHaveCount(0);
      await expect(name).toHaveValue("Pasted title");
      await expect(name).toBeFocused();
      await page.keyboard.press("Escape"); // puts the old name back
      await expect(name).not.toHaveValue("Pasted title");

      // The keyboard: Shift+F10 and the Menu key open it at the focused field; Escape gives the
      // focus back to the field.
      for (const key of ["Shift+F10", "ContextMenu"]) {
        await name.focus();
        await page.keyboard.press(key);
        await expect(page.getByRole("menu", { name: "Text actions for Name" })).toBeVisible();
        await page.keyboard.press("Escape");
        await expect(menu).toHaveCount(0);
        await expect(name).toBeFocused();
      }
    } finally {
      await close();
      work.remove();
    }
  });

  test("the menus of the bin, timeline, ruler, tabs and title bar do what their items say", async () => {
    test.setTimeout(240_000);
    const info = test.info();
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Surfaces", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
      await editorCaughtUp(page);
      const menu = page.getByRole("menu");
      const clipCount = async () =>
        (await sequence(page)).tracks.reduce((n, t) => n + t.items.length, 0);
      const item = (m: ReturnType<typeof page.getByRole>, name: string | RegExp) =>
        m.getByRole("menuitem", { name });

      // A bin card: Insert at playhead places the clip (one undo step: Ctrl+Z takes it back).
      const base = await clipCount();
      await mediaCard(page, "interview.mp4").click({ button: "right" });
      const card = page.getByRole("menu", { name: "Media actions for interview.mp4" });
      await expect(card).toBeVisible();
      await expect(item(card, /^Append to timeline/)).toBeEnabled();
      await expect(item(card, /^Reveal in /)).toBeEnabled();
      await snapshot(page, info, "bin card menu");
      await item(card, /^Insert at playhead/).click();
      await expect(menu).toHaveCount(0);
      await expect.poll(clipCount).toBeGreaterThan(base);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect.poll(clipCount).toBe(base);
      await editorCaughtUp(page);

      // The empty part of the bin: Import… opens the (stubbed) picker and the card arrives.
      const scroll = await page.locator(".cw-media__scroll").boundingBox();
      if (scroll === null) throw new Error("no media list");
      await pickFiles(app, [work.file("broll-city.mp4")]);
      await page.mouse.click(scroll.x + scroll.width - 6, scroll.y + scroll.height - 6, {
        button: "right",
      });
      const bin = page.getByRole("menu", { name: "Media bin actions" });
      await expect(bin).toBeVisible();
      await item(bin, /^Import…/).click();
      await expect(mediaCards(page)).toHaveCount(2);

      // A timeline clip: the right-click selects it; Split at playhead cuts at the playhead;
      // Ctrl+Z puts it back; Disable clip turns it off and the menu then offers Enable.
      let g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(5)) + 1, rulerY(g));
      await expect(timecode(page)).toHaveText("00:00:05:00");
      const clipId = clipAt(g, "V1", 1).id;
      let box = clipBox(g, clipId);
      await page.mouse.click(box.x + Math.min(box.w / 2, 60), box.y, { button: "right" });
      const clipMenu = page.getByRole("menu", { name: "Clip actions for interview.mp4" });
      await expect(clipMenu).toBeVisible();
      await expect(inspector(page).panel).toBeVisible();
      await snapshot(page, info, "timeline clip menu");
      await item(clipMenu, /^Split at playhead/).click();
      await expect
        .poll(async () => (await layout(page))[0]?.clips)
        .toEqual([
          [1, 4],
          [5, 6],
        ]);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
      await editorCaughtUp(page);
      g = await timelineGeometry(page);
      box = clipBox(g, clipId);
      await page.mouse.click(box.x + Math.min(box.w / 2, 60), box.y, { button: "right" });
      await item(clipMenu, /^Disable clip/).click();
      await expect
        .poll(async () => (await sequence(page)).tracks[0]?.items[0]?.enabled)
        .toBe(false);
      await page.mouse.click(box.x + Math.min(box.w / 2, 60), box.y, { button: "right" });
      await expect(item(clipMenu, /^Enable clip/)).toBeVisible();
      await item(clipMenu, /^Enable clip/).click();
      await expect.poll(async () => (await sequence(page)).tracks[0]?.items[0]?.enabled).toBe(true);

      // A track header: Mute (the menu then reads Unmute, checked), Lock, and a drag is refused.
      g = await timelineGeometry(page);
      const header = { x: g.box.x + 12, y: laneY(g, "V1") };
      await page.mouse.click(header.x, header.y, { button: "right" });
      const trackMenu = page.getByRole("menu", { name: "Track actions for V1" });
      await expect(trackMenu).toBeVisible();
      await trackMenu.getByRole("menuitemcheckbox", { name: /^Mute/ }).click();
      // The menu shows the engine's answer: reopen it until the new state has come back.
      const reopenWith = async (row: RegExp) => {
        await expect(async () => {
          await page.keyboard.press("Escape");
          await page.mouse.click(header.x, header.y, { button: "right" });
          await expect(trackMenu.getByRole("menuitemcheckbox", { name: row })).toBeChecked({
            timeout: 1500,
          });
        }).toPass();
      };
      await reopenWith(/^Unmute/);
      await trackMenu.getByRole("menuitemcheckbox", { name: /^Lock/ }).click();
      await reopenWith(/^Unlock/);
      await page.keyboard.press("Escape");
      await expect(menu).toHaveCount(0);
      const before = await layout(page);
      box = clipBox(g, clipId);
      await drag(
        page,
        { x: box.x + Math.min(box.w / 2, 60), y: box.y },
        { x: box.x + Math.min(box.w / 2, 60) + 2 * g.pxPerSec, y: box.y },
      );
      await expect(toast(page, "Clip not moved")).toBeVisible();
      expect(await layout(page)).toEqual(before);
      // A locked track disables the editing rows of its clips, with the reason.
      await page.mouse.click(box.x + Math.min(box.w / 2, 60), box.y, { button: "right" });
      await expect(item(clipMenu, /^Delete clip/)).toBeDisabled();
      await expect(clipMenu.getByText("Track is locked").first()).toBeAttached();
      await page.keyboard.press("Escape");
      await page.mouse.click(header.x, header.y, { button: "right" });
      await trackMenu.getByRole("menuitemcheckbox", { name: /^Unlock/ }).click();
      await expect(menu).toHaveCount(0);

      // An empty lane: Add track above puts one more track on the timeline (one undo step).
      const tracksBefore = (await sequence(page)).tracks.length;
      // (left of the clips, which start at 1 s: the right-click lands on the lane, not on a clip)
      await page.mouse.click(g.box.x + g.headerWidth + 40, laneY(g, "A1"), { button: "right" });
      const lane = page.getByRole("menu", { name: "Timeline actions" });
      await expect(lane).toBeVisible();
      await item(lane, /^Add track above/).click();
      await expect.poll(async () => (await sequence(page)).tracks.length).toBe(tracksBefore + 1);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await sequence(page)).tracks.length).toBe(tracksBefore);
      await editorCaughtUp(page);

      // The ruler: Move playhead here.
      g = await timelineGeometry(page);
      // Two seconds in: on screen at any zoom, and away from the playhead (which is at 5 s).
      const at = 2;
      await page.mouse.click(xAt(g, sec(at)) + 1, rulerY(g), { button: "right" });
      const ruler = page.getByRole("menu", { name: "Ruler actions" });
      await expect(ruler).toBeVisible();
      await item(ruler, /^Move playhead here/).click();
      await expect(timecode(page)).toHaveText(`00:00:${String(at).padStart(2, "0")}:00`);

      // The title bar: the save status offers Save now, and no unsaved mark is left.
      await saveStatus(page).click({ button: "right" });
      const save = page.getByRole("menu", { name: "Save actions" });
      await expect(save).toBeVisible();
      await item(save, /^Save now/).click();
      await expect(dirtyDot(page)).toBeHidden();

      // A dock tab: its menu offers the panel actions; Reset layout is among them.
      await page.getByRole("tab", { name: "Timeline" }).click({ button: "right" });
      const tab = page.getByRole("menu", { name: "Panel actions for Timeline" });
      await expect(tab).toBeVisible();
      for (const name of [
        /^Close panel/,
        /^Maximize panel/,
        /^Float panel/,
        /^Move to/,
        /^Reset layout/,
      ]) {
        await expect(item(tab, name)).toBeVisible();
      }
      await page.keyboard.press("Escape");
      await expect(menu).toHaveCount(0);

      // The keyboard: the Menu key on a focused bin card offers Remove from project; a used
      // asset asks first, and Cancel leaves it in the project.
      await mediaCard(page, "interview.mp4").focus();
      await page.keyboard.press("ContextMenu");
      const keyed = page.getByRole("menu", { name: "Media actions for interview.mp4" });
      await expect(keyed).toBeVisible();
      await item(keyed, /^Remove from project/).click();
      const ask = page.getByRole("dialog", { name: "Remove interview.mp4?" });
      await expect(ask).toBeVisible();
      await ask.getByRole("button", { name: "Cancel" }).click();
      await expect(ask).toBeHidden();
      expect(await mediaNames(page)).toContain("interview.mp4");
    } finally {
      await close();
      work.remove();
    }
  });

  test("the Export chip's menu cancels a running export", async () => {
    test.setTimeout(240_000);
    const corpus = corpusDir();
    test.skip(corpus === null, "the corpus is not fetched (just corpus)");
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work, undefined, { realMedia: true });
    try {
      await createProject(app, page, "Cancel", work.dir);
      const files = copyCorpusClips(corpus ?? "", work.media, ["h264_1080p60.mp4"]);
      await pickFiles(app, files);
      await page.getByRole("button", { name: "Import", exact: true }).click();
      const refused = page.getByText(/not imported/).first();
      await expect(mediaCards(page).first().or(refused)).toBeVisible();
      test.skip(
        await refused.isVisible(),
        "this engine cannot read media; build it with FFmpeg and the real media wiring (WP-1I.05)",
      );
      await dropCard(page, "h264_1080p60.mp4", "V1", 0);
      await expect.poll(async () => (await layout(page))[0]?.clips.length).toBe(1);
      await editorCaughtUp(page);

      // A large software export, so it is still running when the menu is used.
      await page.getByRole("button", { name: "Export", exact: true }).click();
      const dialog = page.getByRole("dialog", { name: "Export video" });
      await expect(dialog).toBeVisible();
      await dialog.getByLabel("Width").fill("3840");
      await dialog.getByLabel("Height").fill("2160");
      await dialog.getByRole("radio", { name: "Low" }).click();
      await dialog.getByRole("switch", { name: "Use hardware encoder" }).click();
      await dialog.getByLabel("Save to").fill(`${work.dir}/Cancel/cancelled.mp4`);
      await dialog.getByRole("button", { name: "Export", exact: true }).click();
      const progress = page.getByRole("dialog", { name: /^Exporting / });
      await expect(progress.or(dialog.getByRole("alert"))).toBeVisible();
      test.skip(
        await dialog.getByRole("alert").isVisible(),
        "this engine cannot export; build it with --features cutweave-engine/ffmpeg (WP-1I.05)",
      );
      await progress.getByRole("button", { name: "Hide" }).click();
      const chip = page.getByTestId("export-chip");
      await expect(chip).toBeVisible();
      await chip.click({ button: "right" });
      const exportMenu = page.getByRole("menu", { name: "Export actions" });
      await expect(exportMenu).toBeVisible();
      await exportMenu.getByRole("menuitem", { name: /^Cancel export/ }).click();
      await expect(toast(page, "Export cancelled")).toBeVisible();
      await expect(chip).toBeHidden();
    } finally {
      await close();
      work.remove();
    }
  });
});
