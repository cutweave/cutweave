// SPDX-License-Identifier: GPL-3.0-or-later
// F47 Detached panels (docs/user-flows.md, WP-2H.14, ADR-0031): move the Inspector into its own OS
// window from its tab's menu, edit a clip there, switch the theme, close that window to dock it
// back, move it out again and relaunch (the window comes back), then Reset layout. The engine's
// focus record (ADR-0021) does not change while the focus moves between the editor and the
// detached window. Playwright drives each window as its own page.
//
// WP-2H.15 extends it: with the Timeline and the Preview each in a window of its own, Space
// pressed in the detached Timeline plays (the timecode runs, the detached Preview presents new
// frames, and drops no more than the playback probe allows), a right-click on a clip there opens
// the clip's menu in that window (Split at playhead, then Ctrl+Z there), Ctrl+K opens the palette
// in that window, a notice raised there shows there, a text field's Paste there works, and axe finds nothing in either detached window. The opt-in real-input steps
// (a card dragged from the main window onto the detached Timeline; the Timeline's tab dragged
// back into the main dock) run with `CUTWEAVE_E2E_REAL_INPUT=1`.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import path from "node:path";
import type { ElectronApplication, Locator, Page } from "@playwright/test";
import { desktopDir, editorCall, expect, quitApp, test } from "../fixtures.ts";
import { copyCorpusClips, corpusDir } from "./export-helpers.ts";
import {
  chooseMenu,
  chooseTheme,
  clipAt,
  clipBox,
  createProject,
  drag,
  dropCard,
  editorCaughtUp,
  freeMainTrack,
  hasRealEngine,
  importWithButton,
  inspector,
  item,
  laneY,
  launchFlowApp,
  layout,
  mediaCard,
  mediaCards,
  pickFiles,
  profileDir,
  removeDir,
  revealTime,
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
} from "./helpers.ts";
import { osInputAvailable, ownerOf, runOsInput, screenPoint } from "./os-input.ts";

/** A panel's content area (a dockview tab panel), in the window `page` shows. */
const panel = (page: Page, name: string): Locator =>
  page.getByRole("tabpanel", { name, exact: true });

/** A panel's box once the dock has stopped resizing it (two reads in a row agree). */
async function settledBox(page: Page, name: string) {
  let last = "";
  let found: { x: number; y: number; width: number; height: number } | null = null;
  await expect
    .poll(async () => {
      found = await panel(page, name).boundingBox();
      const now = found === null ? "" : `${found.x}|${found.y}|${found.width}|${found.height}`;
      const same = now !== "" && now === last;
      last = now;
      return same;
    })
    .toBe(true);
  if (found === null) throw new Error(`the ${name} panel is not visible`);
  return found as { x: number; y: number; width: number; height: number };
}

/** The corpus clip of the WP-2H.15 steps (it has motion to see playback by). */
const CLIP = "sintel_trailer_480p.mp4";

/** `HH:MM:SS:FF` → frames at 30 fps (the new project's rate). */
async function timecodeFrames(page: Page): Promise<number> {
  const [hh, mm, ss, ff] = ((await timecode(page).textContent()) ?? "").split(/[:;]/).map(Number);
  return (((hh ?? 0) * 60 + (mm ?? 0)) * 60 + (ss ?? 0)) * 30 + (ff ?? 0);
}

/** A hash of a 64 × 36 patch at the preview canvas's centre (whichever path drew it). */
async function previewHash(page: Page): Promise<string> {
  const box = await page.locator("canvas.cw-preview__canvas").boundingBox();
  if (box === null) throw new Error("no preview canvas");
  const png = await page.screenshot({
    clip: {
      x: Math.round(box.x + box.width / 2 - 32),
      y: Math.round(box.y + box.height / 2 - 18),
      width: 64,
      height: 36,
    },
    animations: "allow",
  });
  return createHash("sha1").update(png).digest("hex");
}

/** Resolves once `read` gives the same answer twice in a row (the value settled). */
async function settled<T>(read: () => Promise<T>): Promise<T> {
  let last: T | undefined;
  let value: T | undefined;
  await expect
    .poll(
      async () => {
        value = await read();
        const same = value === last;
        last = value;
        return same;
      },
      { intervals: [150, 250, 400] },
    )
    .toBe(true);
  return value as T;
}

/** The preload's present-loop figures (the playback probe), read in the editor window. */
const presentStats = (page: Page) => page.evaluate(() => window.cutweave.preview.getPresentStats());

/** axe's findings in a window's document, as `rule: first node`. */
async function axeViolations(page: Page): Promise<string[]> {
  // The editor's own axe-core (a devDependency there): evaluated in that window, past its CSP.
  const source = readFileSync(
    createRequire(path.join(desktopDir, "..", "..", "packages", "editor", "package.json")).resolve(
      "axe-core/axe.min.js",
    ),
    "utf8",
  );
  await page.evaluate(source);
  return page.evaluate(async () => {
    const axe = (globalThis as unknown as { axe: { run(context: Document): Promise<AxeResult> } })
      .axe;
    interface AxeResult {
      violations: { id: string; nodes: { html: string }[] }[];
    }
    const results = await axe.run(document);
    return results.violations.map((v) => `${v.id}: ${v.nodes[0]?.html ?? ""}`);
  });
}

/**
 * A person's drag with the real mouse from one window to another: press, nudge past the OS drag
 * threshold, move in steps, pause over the target, release (HTML drag and drop runs the OS's
 * drag loop, which needs real input).
 */
function osHandDrag(owner: number, from: { x: number; y: number }, to: typeof from): void {
  const steps: Parameters<typeof runOsInput>[0][number][] = [
    { op: "move", x: from.x, y: from.y },
    { op: "pause", ms: 200 },
    { op: "down" },
    { op: "pause", ms: 200 },
    { op: "move", x: from.x + 12, y: from.y + 12 },
    { op: "pause", ms: 300 },
  ];
  for (let i = 1; i <= 16; i += 1) {
    steps.push({
      op: "move",
      x: Math.round(from.x + ((to.x - from.x) * i) / 16),
      y: Math.round(from.y + ((to.y - from.y) * i) / 16),
    });
    steps.push({ op: "pause", ms: 50 });
  }
  steps.push({ op: "pause", ms: 500 }, { op: "up" }, { op: "pause", ms: 300 });
  runOsInput(steps, owner);
}

/** The detached windows: every page of the app but the editor's. */
const detachedWindows = (app: ElectronApplication): Page[] =>
  app.windows().filter((w) => w.url().endsWith("/popout.html"));

/** Right-click the panel's tab, choose Move to new window: the window that opens. */
async function moveToNewWindow(app: ElectronApplication, page: Page, name: string): Promise<Page> {
  const opened = app.waitForEvent("window");
  await page.getByRole("tab", { name, exact: true }).click({ button: "right" });
  const menu = page.getByRole("menu", { name: `Panel actions for ${name}` });
  await expect(menu).toBeVisible();
  await menu.getByRole("menuitem", { name: /^Move to new window/ }).click();
  const popout = await opened;
  await popout.waitForLoadState("domcontentloaded");
  // On screen only once the panel is in it (WP-2H.15): never an empty window.
  await expect(panel(popout, name)).toBeVisible();
  await expect.poll(() => shownWindowTitles(app)).toContain(await popout.title());
  return popout;
}

/** The titles of the app's windows that are on screen, as main sees them. */
const shownWindowTitles = (app: ElectronApplication) =>
  app.evaluate(({ BrowserWindow }) =>
    BrowserWindow.getAllWindows()
      .filter((w) => w.isVisible())
      .map((w) => w.webContents.getTitle()),
  );

/** The engine's run-file record of when its window last had the focus (ms since the epoch). */
async function focusedAt(app: ElectronApplication, home: string): Promise<number | null> {
  const pid = await app.evaluate(() => globalThis.__cutweaveTest?.enginePid() ?? null);
  if (pid === null) return null;
  try {
    const file = path.join(home, ".cutweave", "run", `${pid}.json`);
    const record = JSON.parse(readFileSync(file, "utf8")) as { focused_at?: number };
    return record.focused_at ?? null;
  } catch {
    // Being rewritten (the engine writes a temp file and renames it): read it again.
    return null;
  }
}

/** The URL of the window the OS has focused, as main sees it. */
const focusedWindow = (app: ElectronApplication) =>
  app.evaluate(({ BrowserWindow }) => BrowserWindow.getFocusedWindow()?.webContents.getURL() ?? "");

/** The active workspace's saved layout has this many detached windows (the editor stores it). */
const storedPopouts = (page: Page) =>
  page.evaluate(() => {
    const view = JSON.parse(localStorage.getItem("cutweave.view") ?? "{}") as {
      state?: { workspaceId?: string; layouts?: Record<string, { popoutGroups?: unknown[] }> };
    };
    return view.state?.layouts?.[view.state.workspaceId ?? ""]?.popoutGroups?.length ?? 0;
  });

test.describe("F47 detached panels", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");
  // macOS keeps the native application menu, which Playwright cannot open (Reset layout there is
  // the native menu's job, as in F33).
  test.skip(process.platform === "darwin", "the page's menu bar is not shown on macOS");

  test("a panel moves into its own window, works there, docks back and is restored on launch", async () => {
    test.setTimeout(240_000);
    const info = test.info();
    const work = workspace();
    const home = profileDir();
    try {
      const first = await launchFlowApp(work, home);
      const { app, page } = first;
      await freeMainTrack(app, page); // clips stay where they are dropped
      await createProject(app, page, "Detached", work.dir);
      await importWithButton(app, page, [work.file("interview.mp4")]);
      await dropCard(page, "interview.mp4", "V1", 1);
      await expect.poll(async () => (await layout(page))[0]?.clips).toEqual([[1, 10]]);
      await editorCaughtUp(page);

      // 1. Right-click the Inspector's tab, Move to new window: a window of its own, named for
      //    it, shows the Inspector; the main window's Preview takes its room.
      const previewBefore = (await settledBox(page, "Preview")).width;
      const inspectorBefore = await settledBox(page, "Inspector");
      let popout = await moveToNewWindow(app, page, "Inspector");
      await expect(popout).toHaveTitle("Inspector — Detached — Cutweave");
      await expect(panel(popout, "Inspector")).toBeVisible();
      await expect(panel(page, "Inspector")).toHaveCount(0);
      await expect
        .poll(async () => (await settledBox(page, "Preview")).width)
        .toBeGreaterThan(previewBefore);
      expect(detachedWindows(app)).toHaveLength(1);
      await snapshot(popout, info, "inspector in its own window");

      // 2. Click the clip in the main window's timeline: the detached Inspector shows it. Its
      //    opacity, dragged there, reaches the engine.
      const id = clipAt(await timelineGeometry(page), "V1", 1).id;
      const cb = clipBox(await timelineGeometry(page), id);
      await page.mouse.click(cb.x + Math.min(cb.w / 2, 60), cb.y);
      const f = inspector(popout);
      await expect(f.panel).toBeVisible();
      await expect(f.duration).toHaveValue("00:00:10:00");
      const rail = f.panel.locator(".cw-slider__track");
      const box = await rail.boundingBox();
      const thumb = await f.opacity.boundingBox();
      if (box === null || thumb === null) throw new Error("no opacity slider");
      await popout.mouse.move(thumb.x + thumb.width / 2, thumb.y + thumb.height / 2);
      await popout.mouse.down();
      await popout.mouse.move(box.x + box.width / 2, box.y + box.height / 2, { steps: 10 });
      await popout.mouse.up();
      await expect.poll(async () => (await item(page, id)).visual.opacity).toBeLessThan(0.65);
      const dragged = Math.round((await item(page, id)).visual.opacity * 100);
      await expect(f.opacity).toHaveAttribute("aria-valuetext", `${dragged}%`);

      // The text actions work in the detached window too (#191): Paste from a field's menu there
      // puts the clipboard in that window's field; Escape gives the old name back.
      await app.evaluate(({ clipboard }, text) => clipboard.writeText(text), "Pasted in popout");
      await f.name.click();
      await popout.keyboard.press("Control+A");
      await f.name.click({ button: "right" });
      const fieldMenu = popout.getByRole("menu", { name: "Text actions for Name" });
      await expect(fieldMenu).toBeVisible();
      await fieldMenu.getByRole("menuitem", { name: "Paste" }).click();
      await expect(f.name).toHaveValue("Pasted in popout");
      await expect(page.getByRole("menu")).toHaveCount(0);
      await popout.keyboard.press("Escape");
      await expect(f.name).not.toHaveValue("Pasted in popout");

      // The focus moving between the editor and its detached window is one window's focus to
      // the engine (ADR-0021): its record of the last focus does not change.
      await page.bringToFront();
      await expect.poll(() => focusedWindow(app)).toBe("app://cutweave/");
      await expect.poll(() => focusedAt(app, home)).not.toBeNull();
      const record = await focusedAt(app, home);
      await popout.bringToFront();
      await expect.poll(() => focusedWindow(app)).toBe("app://cutweave/popout.html");
      await page.bringToFront();
      await expect.poll(() => focusedWindow(app)).toBe("app://cutweave/");
      const reads: (number | null)[] = [];
      await expect
        .poll(
          async () => {
            reads.push(await focusedAt(app, home));
            return reads.slice(-4);
          },
          { intervals: [150] },
        )
        .toEqual([record, record, record, record]);

      // 3. The light theme: the detached window follows.
      await chooseTheme(page, app, "Light");
      await expect(popout.locator("html")).toHaveAttribute("data-theme", "light");
      const background = (p: Page) =>
        p.evaluate(() => getComputedStyle(document.body).backgroundColor);
      await expect.poll(() => background(popout)).toBe(await background(page));
      await snapshot(popout, info, "detached window in the light theme");

      // 4. Close the detached window (the OS close request its close button sends): the
      //    Inspector is docked where it was, right of the Preview at its width.
      const closed = popout.waitForEvent("close");
      await app.evaluate(({ BrowserWindow }) => {
        const win = BrowserWindow.getAllWindows().find((w) =>
          w.webContents.getURL().endsWith("/popout.html"),
        );
        if (win === undefined) throw new Error("no detached window");
        win.close();
      });
      await closed;
      await expect(panel(page, "Inspector")).toBeVisible();
      const inspectorAfter = await settledBox(page, "Inspector");
      const preview = await settledBox(page, "Preview");
      expect(inspectorAfter.x).toBeGreaterThan(preview.x + preview.width - 1);
      expect(Math.abs(inspectorAfter.width - inspectorBefore.width)).toBeLessThan(8);
      expect(detachedWindows(app)).toHaveLength(0);
      await expect.poll(() => storedPopouts(page)).toBe(0);

      // 5. Move it out again and relaunch: the window comes back with the Inspector.
      popout = await moveToNewWindow(app, page, "Inspector");
      await expect(panel(popout, "Inspector")).toBeVisible();
      await expect.poll(() => storedPopouts(page)).toBe(1);
      await quitApp(app, page, "save", first.mainLog);
      await first.close();

      const second = await launchFlowApp(work, home);
      try {
        const page2 = second.page;
        await expect.poll(() => detachedWindows(second.app).length).toBe(1);
        const reopened = detachedWindows(second.app)[0] as Page;
        await expect(panel(reopened, "Inspector")).toBeVisible();
        await expect(reopened).toHaveTitle(/^Inspector — .*Cutweave$/);
        // On screen once the Inspector is back in it (WP-2H.15).
        await expect.poll(() => shownWindowTitles(second.app)).toContain(await reopened.title());
        await expect(reopened.locator("html")).toHaveAttribute("data-theme", "light");
        await expect(panel(page2, "Inspector")).toHaveCount(0);
        await expect(panel(page2, "Timeline")).toBeVisible();
        await snapshot(page2, info, "after relaunch, main window");
        await snapshot(reopened, info, "after relaunch, detached window");

        // 6. View > Reset layout: the window closes and the Inspector docks back.
        await chooseMenu(page2, second.app, "View", "Reset layout");
        await expect(panel(page2, "Inspector")).toBeVisible();
        await expect.poll(() => detachedWindows(second.app).length).toBe(0);
        await expect.poll(() => storedPopouts(page2)).toBe(0);
      } finally {
        // 7. Quitting leaves no window behind (quitApp waits for the app to exit).
        await second.close();
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });

  // WP-2H.15: the Timeline and the Preview, each in a window of its own, work as docked: keys,
  // menus, the palette and playback. Real media (FFmpeg and the corpus) like F13: the preview
  // plays real frames through the ring.
  test("the Timeline and the Preview work in their own windows: keys, menus, the palette, playback", async () => {
    test.setTimeout(300_000);
    const info = test.info();
    const corpus = corpusDir();
    test.skip(corpus === null, "the corpus is not fetched (just corpus)");
    const work = workspace();
    const home = profileDir();
    try {
      const launched = await launchFlowApp(work, home, { realMedia: true });
      const { app, page } = launched;
      try {
        const caps = await editorCall<{ available?: boolean }>(page, "media.capabilities");
        test.skip(
          caps.available !== true,
          "this engine has no media backend; build it with --features cutweave-engine/ffmpeg",
        );
        await createProject(app, page, "Two windows", work.dir);
        await pickFiles(app, copyCorpusClips(corpus ?? "", work.media, [CLIP]));
        await page.getByRole("button", { name: "Import", exact: true }).click();
        await expect(mediaCards(page)).toHaveCount(1);
        await dropCard(page, CLIP, "V1", 0);
        await expect.poll(async () => (await layout(page))[0]?.clips.length ?? 0).toBe(1);
        await editorCaughtUp(page);
        const surface = page.locator(".cw-preview__surface");
        await expect(surface).toHaveAttribute("data-path", /^(gpu|jpeg)$/, { timeout: 20_000 });
        const gpu = (await surface.getAttribute("data-path")) === "gpu";
        await expect(surface).toHaveAttribute("data-live", "true", { timeout: 30_000 });

        // 1. Detach the Timeline and the Preview: a window each, named for its panel.
        const timeline = await moveToNewWindow(app, page, "Timeline");
        const timelineTitle = "Timeline — Two windows — Cutweave";
        await expect(timeline).toHaveTitle(timelineTitle);
        await expect(timelineCanvas(timeline)).toBeVisible();
        const preview = await moveToNewWindow(app, page, "Preview");
        await expect(preview).toHaveTitle("Preview — Two windows — Cutweave");
        await expect(preview.locator(".cw-preview__surface")).toHaveAttribute("data-live", "true");
        expect(detachedWindows(app)).toHaveLength(2);
        await expect(panel(page, "Timeline")).toHaveCount(0);
        await expect(panel(page, "Preview")).toHaveCount(0);

        // 2. Click the detached Timeline's ruler at 10 s and press Space there: the timecode
        //    runs, and the detached Preview's frames change. Space again pauses.
        let g = await timelineGeometry(page, timeline);
        await timeline.mouse.click(xAt(g, sec(10)), rulerY(g));
        await expect(timecode(preview)).toHaveText("00:00:10:00");
        const atTen = await settled(() => previewHash(preview));
        const before = await presentStats(page);
        await shortcut(app, "Space", timelineTitle);
        await expect(preview.getByRole("button", { name: "Pause", exact: true })).toBeVisible();
        await expect
          .poll(() => timecodeFrames(preview), { timeout: 20_000 })
          .toBeGreaterThan(300 + 15);
        await expect.poll(() => previewHash(preview)).not.toBe(atTen);
        if (gpu) {
          // The frames presented in the detached window come from the ring, newer each time.
          await expect
            .poll(async () => (await presentStats(page))?.lastPts ?? 0)
            .toBeGreaterThan(before?.lastPts ?? 0);
          // The playback probe: over 10 s of display frames, fewer than 2 dropped.
          const start = await presentStats(page);
          if (start === null) throw new Error("no present stats while playing");
          await expect
            .poll(async () => (await presentStats(page))?.ticks ?? 0, { timeout: 40_000 })
            .toBeGreaterThanOrEqual(start.ticks + 600);
          const end = await presentStats(page);
          const dropped = (end?.seqGaps ?? 0) - start.seqGaps;
          info.annotations.push({
            type: "playback probe (Preview detached)",
            description: `${dropped} frames dropped over ${(end?.ticks ?? 0) - start.ticks} ticks`,
          });
          expect(dropped).toBeLessThan(2);
        }
        await snapshot(preview, info, "detached preview playing");
        await shortcut(app, "Space", timelineTitle);
        await expect(preview.getByRole("button", { name: "Play", exact: true })).toBeVisible();

        // 3. Right-click the clip in the detached Timeline: its menu opens in that window. Split
        //    at playhead cuts it there; Ctrl+Z pressed there puts it back.
        //    The timeline followed the playhead while it played (WP-2A.13): scroll back to the
        //    clip's start first.
        g = await revealTime(page, 0, timeline);
        const clipId = clipAt(g, "V1", 0).id;
        const box = clipBox(g, clipId);
        await timeline.mouse.click(box.x + Math.min(box.w / 2, 60), box.y, { button: "right" });
        const clipMenu = timeline.getByRole("menu", { name: `Clip actions for ${CLIP}` });
        await expect(clipMenu).toBeVisible();
        await expect(page.getByRole("menu")).toHaveCount(0);
        await snapshot(timeline, info, "clip menu in the detached timeline");
        await clipMenu.getByRole("menuitem", { name: /^Split at playhead/ }).click();
        await expect.poll(async () => (await layout(page))[0]?.clips.length).toBe(2);
        await editorCaughtUp(page);
        await shortcut(app, "Control+Z", timelineTitle);
        await expect.poll(async () => (await layout(page))[0]?.clips.length).toBe(1);
        await editorCaughtUp(page);

        // 4. Ctrl+K there: the palette opens in that window, its search box focused; Esc closes.
        await shortcut(app, "Control+K", timelineTitle);
        const palette = timeline.getByRole("dialog", { name: "Command palette" });
        await expect(palette).toBeVisible();
        await expect(palette.getByRole("combobox", { name: "Search commands" })).toBeFocused();
        await expect(page.getByRole("dialog", { name: "Command palette" })).toHaveCount(0);
        await snapshot(timeline, info, "palette in the detached timeline");
        await shortcut(app, "Escape", timelineTitle);
        await expect(palette).toHaveCount(0);

        // 5. A notice raised in the detached Timeline shows in that window, not in the main one
        //    (#191): drag a clip on a locked track there, and "Clip not moved" appears in it.
        g = await revealTime(page, 0, timeline);
        const header = { x: g.box.x + 12, y: laneY(g, "V1") };
        await timeline.mouse.click(header.x, header.y, { button: "right" });
        const trackMenu = timeline.getByRole("menu", { name: "Track actions for V1" });
        await expect(trackMenu).toBeVisible();
        await trackMenu.getByRole("menuitemcheckbox", { name: /^Lock/ }).click();
        await expect(timeline.getByRole("menu")).toHaveCount(0);
        await expect(async () => {
          await timeline.mouse.click(header.x, header.y, { button: "right" });
          await expect(trackMenu.getByRole("menuitemcheckbox", { name: /^Unlock/ })).toBeChecked({
            timeout: 1500,
          });
        }).toPass({ timeout: 10_000 });
        await timeline.keyboard.press("Escape");
        await expect(timeline.getByRole("menu")).toHaveCount(0);
        const lockedBox = clipBox(await revealTime(page, 0, timeline), clipId);
        const layoutBefore = await layout(page);
        await drag(
          timeline,
          { x: lockedBox.x + Math.min(lockedBox.w / 2, 60), y: lockedBox.y },
          { x: lockedBox.x + Math.min(lockedBox.w / 2, 60) + 2 * g.pxPerSec, y: lockedBox.y },
        );
        await expect(toast(timeline, "Clip not moved")).toBeVisible();
        await expect(toast(page, "Clip not moved")).toHaveCount(0);
        expect(await layout(page)).toEqual(layoutBefore);
        await snapshot(timeline, info, "notice in the detached timeline");
        await toast(timeline, "Clip not moved").getByRole("button", { name: "Dismiss" }).click();
        await expect(toast(timeline, "Clip not moved")).toHaveCount(0);
        await timeline.mouse.click(header.x, header.y, { button: "right" });
        await trackMenu.getByRole("menuitemcheckbox", { name: /^Unlock/ }).click();
        await expect(timeline.getByRole("menu")).toHaveCount(0);

        // axe finds nothing in either detached window.
        expect(await axeViolations(timeline)).toEqual([]);
        expect(await axeViolations(preview)).toEqual([]);

        // Opt-in real input (WP-2H.15): a person's mouse across two OS windows, which Playwright
        // cannot drive (`just e2e-real-input`; Windows only).
        if (osInputAvailable) {
          const owner = await ownerOf(app);
          // A card dragged from the main window's bin onto the detached Timeline lands at the
          // drop point: 5 s, on a track of its own since Video 1 is taken there (as in F4).
          g = await timelineGeometry(page, timeline);
          const card = await mediaCard(page, CLIP).boundingBox();
          if (card === null) throw new Error("no media card");
          const from = await screenPoint(app, {
            x: card.x + card.width / 2,
            y: card.y + card.height / 2,
          });
          const to = await screenPoint(
            app,
            { x: xAt(g, sec(5)), y: laneY(g, "V1") },
            timelineTitle,
          );
          const clipsBefore = (await layout(page)).flatMap((t) => t.clips).length;
          osHandDrag(owner, from, to);
          await expect
            .poll(async () => (await layout(page)).flatMap((t) => t.clips).length)
            .toBeGreaterThan(clipsBefore);
          const starts = (await layout(page)).flatMap((t) => t.clips.map(([at]) => at));
          expect(starts.some((at) => Math.abs(at - 5) < 0.1)).toBe(true);
          // The detached Timeline's tab dragged back into the main window's dock: it docks
          // there, and its window closes.
          const tab = await timeline
            .getByRole("tab", { name: "Timeline", exact: true })
            .boundingBox();
          const dock = await panel(page, "Inspector").boundingBox();
          if (tab === null || dock === null) throw new Error("no tab or dock to drag between");
          osHandDrag(
            owner,
            await screenPoint(
              app,
              { x: tab.x + tab.width / 2, y: tab.y + tab.height / 2 },
              timelineTitle,
            ),
            await screenPoint(app, { x: dock.x + dock.width / 2, y: dock.y + dock.height - 8 }),
          );
          await expect(panel(page, "Timeline")).toBeVisible();
          await expect.poll(() => detachedWindows(app).length).toBe(1);
        } else {
          info.annotations.push({
            type: "skipped",
            description: "real-input steps: set CUTWEAVE_E2E_REAL_INPUT=1 (just e2e-real-input)",
          });
        }
        // Quitting leaves no window behind, the detached ones included.
        await quitApp(app, page, "discard", launched.mainLog);
      } finally {
        // After a failed step the first failure is the one to report, not the quit's.
        await launched.close().catch((error: unknown) => {
          console.warn(`F47: closing after a failed step: ${String(error)}`);
        });
      }
    } finally {
      work.remove();
      removeDir(home);
    }
  });
});
