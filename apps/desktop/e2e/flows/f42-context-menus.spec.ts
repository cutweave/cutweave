// SPDX-License-Identifier: GPL-3.0-or-later
// F42 Context menus (docs/user-flows.md, WP-2H.12): right-click a text field in the Inspector and
// choose Paste; press Shift+F10 and then the Menu key on the focused field and see the menu open
// at it, with Escape returning the focus; right-click dead space and see nothing. Real right
// clicks and keys; the clipboard is the OS's, filled the way a copy in another app would.
//
// WP-2H.13 adds the second flow: the menus of today's surfaces. Right-click a bin card (Insert at
// playhead), the empty bin (Import…), a timeline clip (it becomes the selection; Split at
// playhead, Ctrl+Z, Disable clip, Reset properties after an opacity nudge, Ctrl+Z), a track header (Mute, Lock, then a drag on it is refused), an
// empty lane (Add track above), the ruler (Move playhead here), a dock tab (Reset layout), the
// save status (Save now); press the Menu key on a bin card and choose Remove from project, then
// cancel its dialog. WP-2A.04 adds a clip's left edge: Trim to playhead, then Ctrl+Z; WP-2A.05
// the clip menu's Unlink clips (Link clips is disabled for a pair already linked), then Ctrl+Z.
// WP-2D.01 adds the clip's Speed ▸ 2x and Custom…, Reverse and Freeze frame, each undone.
// WP-2A.06 adds the clipboard flow: Ctrl+C and Ctrl+V, Ctrl+X, Ctrl+D, the clip menu's Copy and
// Paste attributes… and the lane menu's Paste here, each one undo step. WP-2A.11 adds Replace
// clip… (with a bin card picked, or by asking in a dialog) and the bin card's Replace selected
// clip, each one undo step that keeps the clip, its place and its dimming. A further
// flow right-clicks the Export chip and cancels the export (it needs
// the FFmpeg engine and the corpus, like F14, and skips without them). Every choice is checked as
// an undo step or as state the person sees. Only the OS file picker is stubbed.
import { editorCall, expect, test } from "../fixtures.ts";
import { copyCorpusClips, corpusDir } from "./export-helpers.ts";
import {
  clickClip,
  clipNames,
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
  item as itemOf,
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
  timelineCanvas,
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

      // Links (WP-2A.05): the clip and its sound are one group; Unlink clips takes the picture
      // out (the pair dissolves) and Ctrl+Z links them again.
      await editorCaughtUp(page);
      g = await timelineGeometry(page);
      box = clipBox(g, clipId);
      await expect
        .poll(async () => (await sequence(page)).tracks[0]?.items[0]?.linked_group)
        .not.toBeNull();
      await page.mouse.click(box.x + Math.min(box.w / 2, 60), box.y, { button: "right" });
      await expect(item(clipMenu, /^Link clips/)).toBeDisabled();
      await item(clipMenu, /^Unlink clips/).click();
      await expect
        .poll(async () => (await sequence(page)).tracks[0]?.items[0]?.linked_group)
        .toBeNull();
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect
        .poll(async () => (await sequence(page)).tracks[0]?.items[0]?.linked_group)
        .not.toBeNull();
      await editorCaughtUp(page);

      // Reset properties (WP-2C.03): nudge the opacity down a step in the Inspector, then reset
      // the clip from its menu; Ctrl+Z brings the nudge back.
      const opacity = inspector(page).opacity;
      await opacity.focus();
      await opacity.press("ArrowLeft");
      await expect.poll(async () => (await itemOf(page, clipId)).visual.opacity).toBeLessThan(1);
      await editorCaughtUp(page);
      g = await timelineGeometry(page);
      box = clipBox(g, clipId);
      await page.mouse.click(box.x + Math.min(box.w / 2, 60), box.y, { button: "right" });
      await item(clipMenu, /^Reset properties/).click();
      await expect.poll(async () => (await itemOf(page, clipId)).visual.opacity).toBe(1);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await itemOf(page, clipId)).visual.opacity).toBeLessThan(1);
      await editorCaughtUp(page);

      // Speed (WP-2D.01): Speed ▸ 2x halves the clip and its sound, and Ctrl+Z puts it back;
      // Speed ▸ Custom… asks for a speed and 1/2 doubles it; Reverse plays it backwards (checked
      // the next time the menu opens); Freeze frame at the playhead (5 s) splits the clip there
      // and puts a 3 s still in between while its sound waits. Each is one undo step.
      const openClipMenu = async () => {
        await editorCaughtUp(page);
        g = await timelineGeometry(page);
        box = clipBox(g, clipId);
        await page.mouse.click(box.x + Math.min(box.w / 2, 60), box.y, { button: "right" });
        await expect(clipMenu).toBeVisible();
      };
      const speedMenu = page.getByRole("menu", { name: "Speed" });
      await openClipMenu();
      await item(clipMenu, /^Speed/).click();
      await expect(speedMenu.getByRole("menuitemradio", { name: /^Normal/ })).toBeChecked();
      await snapshot(page, info, "clip speed menu");
      await speedMenu.getByRole("menuitemradio", { name: /^2x/ }).click();
      await expect
        .poll(() => layout(page))
        .toEqual([
          { track: "V1", clips: [[1, 5]] },
          { track: "A1", clips: [[1, 5]] },
        ]);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
      await openClipMenu();
      await item(clipMenu, /^Speed/).click();
      await speedMenu.getByRole("menuitem", { name: /^Custom/ }).click();
      const speedDialog = page.getByRole("dialog", { name: "Custom speed" });
      await expect(speedDialog).toBeVisible();
      await speedDialog.getByRole("textbox", { name: "Speed" }).fill("1/2");
      await speedDialog.getByRole("button", { name: "Set speed" }).click();
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 20]]);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
      await openClipMenu();
      await clipMenu.getByRole("menuitemcheckbox", { name: /^Reverse/ }).click();
      await expect.poll(async () => (await itemOf(page, clipId)).source?.reverse).toBe(true);
      await openClipMenu();
      await expect(clipMenu.getByRole("menuitemcheckbox", { name: /^Reverse/ })).toBeChecked();
      await page.keyboard.press("Escape");
      await expect(menu).toHaveCount(0);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await itemOf(page, clipId)).source?.reverse).toBe(false);
      await openClipMenu();
      await item(clipMenu, /^Freeze frame/).click();
      await expect
        .poll(() => layout(page))
        .toEqual([
          {
            track: "V1",
            clips: [
              [1, 4],
              [5, 3],
              [8, 6],
            ],
          },
          {
            track: "A1",
            clips: [
              [1, 4],
              [8, 6],
            ],
          },
        ]);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);

      // The clip's left edge (WP-2A.04): Trim to playhead moves the start to the playhead (5 s),
      // with its sound; Roll says why it cannot run; Ctrl+Z puts the clip back.
      await editorCaughtUp(page);
      g = await timelineGeometry(page);
      box = clipBox(g, clipId);
      await page.mouse.click(box.x + 2, box.y, { button: "right" });
      const edgeMenu = page.getByRole("menu", { name: "Edge actions for interview.mp4" });
      await expect(edgeMenu).toBeVisible();
      await expect(item(edgeMenu, /^Roll to playhead/)).toBeDisabled();
      await expect(edgeMenu.getByText("No clip touches this edge").first()).toBeAttached();
      await snapshot(page, info, "timeline edge menu");
      await item(edgeMenu, /^Trim to playhead/).click();
      await expect
        .poll(() => layout(page))
        .toEqual([
          { track: "V1", clips: [[5, 6]] },
          { track: "A1", clips: [[5, 6]] },
        ]);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
      await editorCaughtUp(page);

      // A track header: Mute (the menu then reads Unmute, checked), Lock, and a drag is refused.
      g = await timelineGeometry(page);
      const header = { x: g.box.x + 12, y: laneY(g, "V1") };
      // Mute is the audio track's; Lock is V1's.
      const audioHeader = { x: header.x, y: laneY(g, "A1") };
      await page.mouse.click(audioHeader.x, audioHeader.y, { button: "right" });
      const audioMenu = page.getByRole("menu", { name: "Track actions for A1" });
      await expect(audioMenu).toBeVisible();
      await audioMenu.getByRole("menuitemcheckbox", { name: /^Mute/ }).click();
      await expect(menu).toHaveCount(0); // the chosen command runs once the menu has closed
      // The menu shows the engine's answer: reopen it until the new state has come back.
      const reopenWith = async (
        at: { x: number; y: number },
        menuOf: typeof audioMenu,
        row: RegExp,
      ) => {
        await expect(async () => {
          await page.mouse.click(at.x, at.y, { button: "right" });
          await expect(menuOf.getByRole("menuitemcheckbox", { name: row })).toBeChecked({
            timeout: 1500,
          });
        }).toPass({ timeout: 10_000 });
      };
      await reopenWith(audioHeader, audioMenu, /^Unmute/);
      await page.keyboard.press("Escape");
      await page.mouse.click(header.x, header.y, { button: "right" });
      const trackMenu = page.getByRole("menu", { name: "Track actions for V1" });
      await expect(trackMenu).toBeVisible();
      await trackMenu.getByRole("menuitemcheckbox", { name: /^Lock/ }).click();
      await expect(menu).toHaveCount(0); // the chosen command runs once the menu has closed
      await reopenWith(header, trackMenu, /^Unlock/);
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
        // Its own window (WP-2H.14, F47), and the dock's edges (a submenu).
        /^Move to new window/,
        /^Move to$/,
        /^Reset layout/,
      ]) {
        await expect(item(tab, name)).toBeVisible();
      }
      await page.keyboard.press("Escape");
      await expect(menu).toHaveCount(0);

      // The keyboard: the Menu key on a focused bin card offers Remove from project; a used
      // asset asks first, and Cancel leaves it in the project.
      const keyed = page.getByRole("menu", { name: "Media actions for interview.mp4" });
      // (the focus may still be on its way back from the tab's menu: focus the card and press
      // the key until the menu is there)
      await expect(async () => {
        await mediaCard(page, "interview.mp4").focus();
        await expect(mediaCard(page, "interview.mp4")).toBeFocused({ timeout: 1000 });
        await page.keyboard.press("ContextMenu");
        await expect(keyed).toBeVisible({ timeout: 2000 });
      }).toPass({ timeout: 10_000 });
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

  test("copy, cut, paste, duplicate and paste attributes work from the keys and the menus (WP-2A.06)", async () => {
    test.setTimeout(240_000);
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Clipboard", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect
        .poll(() => layout(page))
        .toEqual([
          { track: "V1", clips: [[1, 10]] },
          { track: "A1", clips: [[1, 10]] },
        ]);
      await editorCaughtUp(page);
      const menu = page.getByRole("menu");
      const item = (m: ReturnType<typeof page.getByRole>, name: string | RegExp) =>
        m.getByRole("menuitem", { name });
      const clipsOnV1 = async () => (await layout(page))[0]?.clips;
      const original = (await sequence(page)).tracks[0]?.items[0]?.id ?? "";
      // Zoom out (- on the focused timeline) so the playhead can go well past the clip.
      await timelineCanvas(page).locator("canvas").focus();
      for (let n = 0; n < 5; n += 1) await page.keyboard.press("-");
      await expect
        .poll(async () => (await editorCall<{ zoom: number | null }>(page, "view.get")).zoom)
        .toBeLessThan(35);
      const moveTo = async (s: number) => {
        const g = await timelineGeometry(page);
        await page.mouse.click(xAt(g, sec(s)) + 0.5, rulerY(g));
        await expect(timecode(page)).toHaveText(`00:00:${String(s).padStart(2, "0")}:00`);
      };

      // Ctrl+C then Ctrl+V: the clip and its sound come back at the playhead, on their own
      // tracks and in a group of their own, as one undo step (Ctrl+Z).
      await clickClip(page, original);
      await shortcut(app, "Control+C");
      await moveTo(14);
      await shortcut(app, "Control+V");
      await expect
        .poll(() => layout(page))
        .toEqual([
          {
            track: "V1",
            clips: [
              [1, 10],
              [14, 10],
            ],
          },
          {
            track: "A1",
            clips: [
              [1, 10],
              [14, 10],
            ],
          },
        ]);
      await editorCaughtUp(page);
      const items = (await sequence(page)).tracks[0]?.items ?? [];
      expect(items[1]?.id).not.toBe(original);
      expect(items[1]?.linked_group).not.toBeNull();
      expect(items[1]?.linked_group).not.toBe(items[0]?.linked_group);
      await shortcut(app, "Control+Z");
      await expect.poll(clipsOnV1).toEqual([[1, 10]]);
      await editorCaughtUp(page);

      // Ctrl+X takes the clip out (both tracks); Ctrl+V puts it at the playhead; two undo steps
      // return the first position.
      await clickClip(page, original);
      await shortcut(app, "Control+X");
      await expect
        .poll(() => layout(page))
        .toEqual([
          { track: "V1", clips: [] },
          { track: "A1", clips: [] },
        ]);
      await editorCaughtUp(page);
      await shortcut(app, "Control+V");
      await expect.poll(clipsOnV1).toEqual([[14, 10]]);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await shortcut(app, "Control+Z");
      await expect.poll(clipsOnV1).toEqual([[1, 10]]);
      await editorCaughtUp(page);

      // Ctrl+D copies the clip right after itself.
      await clickClip(page, original);
      await shortcut(app, "Control+D");
      await expect.poll(clipsOnV1).toEqual([
        [1, 10],
        [11, 10],
      ]);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect.poll(clipsOnV1).toEqual([[1, 10]]);
      await editorCaughtUp(page);

      // The menus: Copy on the clip, then Paste here on an empty lane at the click's time.
      let g = await timelineGeometry(page);
      let box = clipBox(g, original);
      await page.mouse.click(box.x + Math.min(box.w / 2, 60), box.y, { button: "right" });
      const clipMenu = page.getByRole("menu", { name: "Clip actions for interview.mp4" });
      await expect(clipMenu).toBeVisible();
      await item(clipMenu, /^Copy(?! path)/).click();
      await expect(menu).toHaveCount(0);
      g = await timelineGeometry(page);
      await page.mouse.click(xAt(g, sec(22)) + 0.5, laneY(g, "V1"), { button: "right" });
      const laneMenu = page.getByRole("menu", { name: "Timeline actions" });
      await expect(laneMenu).toBeVisible();
      await item(laneMenu, /^Paste here/).click();
      await expect.poll(clipsOnV1).toEqual([
        [1, 10],
        [22, 10],
      ]);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect.poll(clipsOnV1).toEqual([[1, 10]]);
      await editorCaughtUp(page);

      // Paste attributes: the copy gets the first clip's opacity; Ctrl+Z takes it back.
      await moveTo(14);
      await shortcut(app, "Control+V");
      await expect.poll(clipsOnV1).toEqual([
        [1, 10],
        [14, 10],
      ]);
      await editorCaughtUp(page);
      const copy = (await sequence(page)).tracks[0]?.items[1]?.id ?? "";
      await clickClip(page, original);
      const opacity = inspector(page).opacity;
      await opacity.focus();
      await opacity.press("ArrowLeft");
      await expect.poll(async () => (await itemOf(page, original)).visual.opacity).toBeLessThan(1);
      await editorCaughtUp(page);
      const dimmed = (await itemOf(page, original)).visual.opacity;
      expect((await itemOf(page, copy)).visual.opacity).toBe(1);
      await clickClip(page, original);
      await shortcut(app, "Control+C");
      g = await timelineGeometry(page);
      box = clipBox(g, copy);
      await page.mouse.click(box.x + Math.min(box.w / 2, 60), box.y, { button: "right" });
      await item(clipMenu, /^Paste attributes/).click();
      const dialog = page.getByRole("dialog", { name: "Paste attributes" });
      await expect(dialog).toBeVisible();
      await dialog.getByRole("button", { name: "Paste", exact: true }).click();
      await expect(dialog).toBeHidden();
      await expect.poll(async () => (await itemOf(page, copy)).visual.opacity).toBe(dimmed);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect.poll(async () => (await itemOf(page, copy)).visual.opacity).toBe(1);
    } finally {
      await close();
      work.remove();
    }
  });

  test("Replace clip… and Replace selected clip swap the media and keep the clip, each one undo step (WP-2A.11)", async () => {
    test.setTimeout(240_000);
    const work = workspace();
    const { app, page, close } = await launchFlowApp(work);
    try {
      await createProject(app, page, "Replace", work.dir);
      await importWithButton(app, page, [
        work.file("interview.mp4"),
        work.file("broll-city.mp4"),
        work.file("music.wav"),
      ]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect.poll(() => clipNames(page, "V1")).toEqual(["interview.mp4"]);
      await expect.poll(() => clipNames(page, "A1")).toEqual(["interview.mp4"]);
      await editorCaughtUp(page);
      const original = (await sequence(page)).tracks[0]?.items[0]?.id ?? "";
      const item = (m: ReturnType<typeof page.getByRole>, name: string | RegExp) =>
        m.getByRole("menuitem", { name });
      const rightClickClip = async () => {
        const g = await timelineGeometry(page);
        const box = clipBox(g, original);
        await page.mouse.click(box.x + Math.min(box.w / 2, 60), box.y, { button: "right" });
      };
      const startsAt1 = async () => (await layout(page))[0]?.clips[0]?.[0];
      // Dim the clip first: the replace must keep it.
      await clickClip(page, original);
      const opacity = inspector(page).opacity;
      await opacity.focus();
      await opacity.press("ArrowLeft");
      await expect.poll(async () => (await itemOf(page, original)).visual.opacity).toBeLessThan(1);
      await editorCaughtUp(page);
      const dimmed = (await itemOf(page, original)).visual.opacity;

      // Replace clip… with only sound picked in the bin: it asks which media, and a still or a
      // video fits; the sound is not offered.
      await mediaCard(page, "music.wav").click();
      await rightClickClip();
      const clipMenu = page.getByRole("menu", { name: "Clip actions for interview.mp4" });
      await expect(clipMenu).toBeVisible();
      await item(clipMenu, /^Replace clip…/).click();
      const dialog = page.getByRole("dialog", { name: "Replace clip" });
      await expect(dialog).toBeVisible();
      await expect(dialog.getByRole("radio", { name: "music.wav" })).toHaveCount(0);
      await dialog.getByRole("radio", { name: "broll-city.mp4" }).check();
      await dialog.getByRole("button", { name: "Replace", exact: true }).click();
      await expect(dialog).toBeHidden();
      await expect.poll(() => clipNames(page, "V1")).toEqual(["broll-city.mp4"]);
      await expect.poll(() => clipNames(page, "A1")).toEqual(["broll-city.mp4"]);
      await editorCaughtUp(page);
      // Same clip, same place, same dimming.
      expect((await sequence(page)).tracks[0]?.items[0]?.id).toBe(original);
      expect(await startsAt1()).toBe(1);
      expect((await itemOf(page, original)).visual.opacity).toBe(dimmed);
      await shortcut(app, "Control+Z");
      await expect.poll(() => clipNames(page, "V1")).toEqual(["interview.mp4"]);
      await expect.poll(() => clipNames(page, "A1")).toEqual(["interview.mp4"]);
      await editorCaughtUp(page);

      // Pick the card, then the clip: Replace clip… swaps at once, with no question.
      await mediaCard(page, "broll-city.mp4").click();
      await rightClickClip();
      await expect(clipMenu).toBeVisible();
      await item(clipMenu, /^Replace clip…/).click();
      await expect.poll(() => clipNames(page, "V1")).toEqual(["broll-city.mp4"]);
      await expect(dialog).toHaveCount(0);
      await editorCaughtUp(page);
      expect((await itemOf(page, original)).visual.opacity).toBe(dimmed);
      await shortcut(app, "Control+Z");
      await expect.poll(() => clipNames(page, "V1")).toEqual(["interview.mp4"]);
      await editorCaughtUp(page);

      // Replace selected clip: pick the clip, then right-click the card.
      await clickClip(page, original);
      await mediaCard(page, "broll-city.mp4").click({ button: "right" });
      const card = page.getByRole("menu", { name: "Media actions for broll-city.mp4" });
      await expect(card).toBeVisible();
      await item(card, /^Replace selected clip/).click();
      await expect.poll(() => clipNames(page, "V1")).toEqual(["broll-city.mp4"]);
      await expect.poll(() => clipNames(page, "A1")).toEqual(["broll-city.mp4"]);
      await editorCaughtUp(page);
      await shortcut(app, "Control+Z");
      await expect.poll(() => clipNames(page, "V1")).toEqual(["interview.mp4"]);
      await expect.poll(() => clipNames(page, "A1")).toEqual(["interview.mp4"]);
      await editorCaughtUp(page);

      // Sound cannot replace a picture: the card says why.
      await mediaCard(page, "music.wav").click({ button: "right" });
      const music = page.getByRole("menu", { name: "Media actions for music.wav" });
      await expect(item(music, /^Replace selected clip/)).toBeDisabled();
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
