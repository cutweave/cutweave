// SPDX-License-Identifier: GPL-3.0-or-later
// Shared helpers of the user-flow suite (ADR-0018, docs/user-flows.md). A flow drives the built
// app the way a person does: pointer clicks and drags on real elements, typing, shortcuts, the
// app's menus and its own dialogs. What these helpers may and may not do:
//
// - Stub only the OS boundary: the native file pickers (`dialog.showOpenDialog`, in main) return
//   the paths a person would pick. Nothing inside the app is bypassed.
// - Read state, never change it: `editorCall` is used only to read the document back
//   (`timeline.get`, `project.info`, `view.get`, `item.get`) or for setup a flow does not test.
// - Menus: every menu action goes through `chooseMenu()`, the one place that knows how the menu
//   is drawn: the menu bar in the title bar on Windows and Linux (WP-1M.09), the native app menu
//   on macOS.
// - Menu shortcuts (Ctrl+S, Ctrl+Z, ...): `shortcut()` sends the key press to the window from main
//   (`webContents.sendInputEvent`), the input path a real key press takes. On Windows and Linux
//   the editor's menu bar handles it; on macOS the native menu's accelerator does. Editor-level
//   keys (Space, J/K/L, arrows, timeline keys, Ctrl+I) go through `page.keyboard`.
// - Wait for visible state, never sleep.
import {
  closeSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import type { ElectronApplication, Locator, Page, TestInfo } from "@playwright/test";
import {
  desktopDir,
  editorCall,
  editorHandle,
  editorState,
  expect,
  type LaunchedApp,
  launchApp,
  REAL_ENGINE_ENV,
} from "../fixtures.ts";

export { hasRealEngine } from "../fixtures.ts";

/** Flicks per second (ADR-0004): every time on the wire is flicks. */
export const FLICKS = 705_600_000;
export const sec = (s: number): number => Math.round(s * FLICKS);
/** One frame at the default 30 fps sequence. */
export const FRAME = FLICKS / 30;

/** The `cutweave` CLI (`cargo build -p cutweave-cli`): F15 attaches it to the app's engine. */
export const cliBin =
  process.env.CUTWEAVE_CLI_BIN ??
  path.join(
    desktopDir,
    "..",
    "..",
    "target",
    "debug",
    process.platform === "win32" ? "cutweave.exe" : "cutweave",
  );
export const hasCli = existsSync(cliBin);

/** The demo media (`cutweave-demo/media`); placeholders stand in when it is not on this machine. */
const DEMO_MEDIA = process.env.CUTWEAVE_DEMO_MEDIA ?? "E:/projects/cutweave-demo/media";
export const DEMO_FILES = [
  "interview.mp4",
  "broll-city.mp4",
  "broll-coffee.mp4",
  "music.wav",
  "logo.png",
] as const;
export type DemoFile = (typeof DEMO_FILES)[number];

/** A throwaway folder for one flow: `media/` holds the demo files, projects go next to it. */
export interface Workspace {
  readonly dir: string;
  readonly media: string;
  file(name: DemoFile): string;
  remove(): void;
}

/**
 * Copies the demo media into a fresh folder. The engine runs with `--dev-stub-media` until
 * WP-1I.05, which probes by extension, so on a machine without the demo folder small placeholder
 * files of the same names stand in (nothing is committed, nothing is decoded).
 */
export function workspace(): Workspace {
  const dir = mkdtempSync(path.join(tmpdir(), "cutweave-flow-"));
  const media = path.join(dir, "media");
  mkdirSync(media);
  const demo = existsSync(DEMO_MEDIA) ? new Set(readdirSync(DEMO_MEDIA)) : new Set<string>();
  for (const name of DEMO_FILES) {
    if (demo.has(name)) copyFileSync(path.join(DEMO_MEDIA, name), path.join(media, name));
    else writeFileSync(path.join(media, name), `placeholder for ${name} (Cutweave e2e)\n`);
  }
  return {
    dir,
    media,
    file: (name) => path.join(media, name),
    remove: () => removeDir(dir),
  };
}

/**
 * Removes a flow's folder. Windows keeps a folder busy for a moment after the processes that
 * used it exit (a killed app's helpers, the virus scanner), so this retries for ten seconds. If
 * it is still locked after that, the folder stays in the temp directory with a warning rather
 * than failing a flow whose checks all passed: a process the suite really left running is
 * caught and failed by the run-level check (`e2e/run-check.ts`).
 */
export function removeDir(dir: string): void {
  try {
    rmSync(dir, { recursive: true, force: true, maxRetries: 40, retryDelay: 250 });
  } catch (error) {
    const code = (error as NodeJS.ErrnoException).code;
    if (code !== "EPERM" && code !== "EBUSY" && code !== "ENOTEMPTY") throw error;
    console.warn(`left ${dir} in the temp directory: still locked (${code})`);
  }
}

/**
 * Launches the built app with the real engine (`cutweave-engine-serve`), stub media probing and
 * the workspace allowed as an engine root, and waits until the editor shows "Engine connected".
 * Pass `home` to relaunch on the same profile (settings, recent projects).
 */
export async function launchFlowApp(
  work: Workspace,
  home?: string,
  options: {
    /**
     * Decode real files: the full engine (WP-1I.05; FFmpeg when it is built with it) with
     * playback, the preview ring and `render.frame` (F13, F14, F17). Its audio plays on the null
     * output, so a run makes no sound. Default: stub probing.
     */
    realMedia?: boolean;
  } = {},
): Promise<LaunchedApp> {
  const launched = await launchApp(
    {
      ...REAL_ENGINE_ENV,
      CUTWEAVE_DEV_STUB_MEDIA: options.realMedia === true ? "" : "1",
      CUTWEAVE_AUDIO: "null",
      // Real-media flows write thumbnails, filmstrips and waveforms: into a throwaway folder of
      // the workspace, never the real user's cache (the engine's default is the OS cache folder).
      // A cache folder chosen in Settings (`--cache-dir`) wins over this default.
      ...(options.realMedia === true
        ? { CUTWEAVE_CACHE_DIR: path.join(work.dir, "default-cache") }
        : {}),
      CUTWEAVE_ENGINE_ALLOW_ROOTS: work.dir,
      // Every launch gets a fresh, empty user profile (fixtures.ts). On Windows, creating a
      // D3D12 device in a profile without `AppData\Local` hangs in the driver (measured > 90 s on
      // the reference laptop; Vulkan opens in about 2 s), so the engine's GPU uses Vulkan here.
      // A real profile is not affected. `CUTWEAVE_FLOW_WGPU_BACKEND` overrides it.
      ...(process.platform === "win32"
        ? { WGPU_BACKEND: process.env.CUTWEAVE_FLOW_WGPU_BACKEND ?? "vulkan" }
        : {}),
    },
    [],
    home === undefined ? {} : { home },
  );
  await editorHandle(launched.page);
  await expect.poll(async () => (await editorState(launched.page)).connection).toBe("ready");
  await expect(engineChip(launched.page)).toHaveText("Engine connected");
  return launched;
}

/** A fresh HOME that survives the app (a relaunch reuses it); remove it when the flow ends. */
export function profileDir(): string {
  return mkdtempSync(path.join(tmpdir(), "cutweave-flow-home-"));
}

// ---- what the user sees ----------------------------------------------------------------------

export const engineChip = (page: Page): Locator => page.locator(".cw-engine-status");
export const dirtyDot = (page: Page): Locator => page.getByTestId("dirty-dot");
export const toolbar = (page: Page): Locator =>
  page.getByRole("toolbar", { name: "Project actions" });
export const mediaCards = (page: Page): Locator =>
  page.getByRole("listbox", { name: "Media files" }).getByRole("option");
export const mediaCard = (page: Page, name: string): Locator =>
  mediaCards(page).filter({ hasText: name });
/** The bin's sidebar (WP-2F.06c): the folder tree and the collections. */
export const binSidebar = (page: Page): Locator =>
  page.getByRole("complementary", { name: "Folders and collections" });

/**
 * Shows the bin's sidebar with its toolbar button, unless it is showing. In a narrow media panel
 * it opens over the grid and closes again once a row is opened, so a flow asks for it each time
 * it needs it.
 */
export async function showBinSidebar(page: Page): Promise<Locator> {
  const side = binSidebar(page);
  if (!(await side.isVisible())) {
    await page.getByRole("button", { name: "Show folders and collections" }).click();
  }
  await expect(side).toBeVisible();
  return side;
}

/** Hides the bin's sidebar with its toolbar button, unless it is hidden already. */
export async function hideBinSidebar(page: Page): Promise<void> {
  const side = binSidebar(page);
  if (await side.isVisible()) {
    await page.getByRole("button", { name: "Hide folders and collections" }).click();
  }
  await expect(side).toBeHidden();
}

/**
 * Drags a bin tile onto a row of the sidebar, holding the tile near its right edge: over a narrow
 * panel the sidebar covers the tile's left part.
 */
export async function dragTileToSidebar(tile: Locator, row: Locator): Promise<void> {
  const box = await tile.boundingBox();
  if (box === null) throw new Error("the tile is not on screen");
  await tile.dragTo(row, { sourcePosition: { x: box.width - 16, y: box.height / 2 } });
}

export const timelineCanvas = (page: Page): Locator =>
  page.getByRole("application", { name: "Timeline" });
export const timelineTools = (page: Page): Locator =>
  page.getByRole("toolbar", { name: "Timeline tools" });
export const timecode = (page: Page): Locator => page.getByLabel("Current time", { exact: true });
/** A toast by its title (the toast itself, not Radix's live-region copy of its text). */
export const toast = (page: Page, title: string | RegExp): Locator =>
  page.locator(".cw-toast", { hasText: title });

/**
 * Screenshot of the window at a key step of the flow: kept in the test's output folder and
 * attached to the report, for a person to look at (not compared against a baseline).
 */
export async function snapshot(page: Page, info: TestInfo, name: string): Promise<void> {
  const file = info.outputPath(`${name.replace(/[^a-z0-9]+/gi, "-")}.png`);
  await page.screenshot({ path: file });
  await info.attach(name, { path: file, contentType: "image/png" });
}

/** Main-process log lines that report a problem (the stub-media warning is expected). */
export function logProblems(mainLog: readonly string[]): string[] {
  return mainLog.filter(
    (line) =>
      /\b(error|panic|panicked|uncaught|unhandled|exception|fatal)\b/i.test(line) &&
      !/dev stub media is on/.test(line),
  );
}

// ---- keyboard and menus ----------------------------------------------------------------------

/** `Control+S` style chords for `shortcut()`; `Control` is Cmd on macOS (`CmdOrCtrl`). */
export type Chord = `${"Control+" | "Control+Shift+" | ""}${string}`;

/**
 * Presses a menu shortcut (`Control+S`, `Control+Shift+S`, `Control+Z`, ...) on the editor
 * window. The key goes in through main (`webContents.sendInputEvent`), as a key press on the
 * window does, so the menu runs the same command a click on its item would.
 *
 * Keys only reach a focused window (Electron drops `sendInputEvent` otherwise), and another
 * app's window can take the focus while the suite runs. A person's keys go to the window they
 * are looking at, so this brings the editor to the front first and waits until it has the focus.
 *
 * `inWindow` is the title of a detached panel's window to press it in instead ("Timeline — My
 * video — Cutweave", WP-2H.15); by default the keys go to the editor window, whatever other
 * windows are open.
 */
export async function shortcut(
  app: ElectronApplication,
  chord: Chord,
  inWindow?: string,
): Promise<void> {
  const parts = chord.split("+");
  const key = parts.pop() ?? "";
  const mac = process.platform === "darwin";
  const modifiers = parts.map((m) =>
    m === "Control" ? (mac ? "meta" : "control") : m.toLowerCase(),
  );
  const focused = () =>
    app.evaluate(({ BrowserWindow }, title) => {
      const win = BrowserWindow.getAllWindows().find((w) =>
        title === null
          ? !w.webContents.getURL().endsWith("/popout.html")
          : w.webContents.getTitle() === title,
      );
      if (win === undefined) throw new Error("no window to type into");
      if (win.isFocused() && win.webContents.isFocused()) return true;
      win.focus();
      win.webContents.focus();
      return false;
    }, inWindow ?? null);
  await expect.poll(focused, { message: "the window never got the focus" }).toBe(true);
  await app.evaluate(
    ({ BrowserWindow }, { key, modifiers, title }) => {
      const win = BrowserWindow.getAllWindows().find((w) =>
        title === null
          ? !w.webContents.getURL().endsWith("/popout.html")
          : w.webContents.getTitle() === title,
      );
      if (win === undefined) throw new Error("no window to type into");
      const mods = modifiers as ("control" | "meta" | "shift")[];
      win.webContents.sendInputEvent({ type: "keyDown", keyCode: key, modifiers: mods });
      win.webContents.sendInputEvent({ type: "keyUp", keyCode: key, modifiers: mods });
    },
    { key, modifiers, title: inWindow ?? null },
  );
}

/**
 * Chooses a menu item (`File` > `New project…`) like a person: on Windows and Linux it clicks the
 * menu in the title bar's menu bar (WP-1M.09), then the item, and waits for the menu to close.
 * Every menu action of the flows goes through here, so only this helper changes when the menu
 * does. `item` is the item's label (its shortcut may follow it), or a pattern for labels that
 * name the step (`/^Undo/` matches "Undo Move clip"). On macOS the native app menu stays, and
 * Playwright cannot open it: the item's handler runs as a click on it would.
 */
export async function chooseMenu(
  page: Page,
  app: ElectronApplication,
  menu: "File" | "Edit" | "View" | "Help",
  item: string | RegExp,
): Promise<void> {
  if (process.platform === "darwin") {
    const pattern = typeof item === "string" ? null : { source: item.source, flags: item.flags };
    const label = typeof item === "string" ? item : null;
    const found = await app.evaluate(
      ({ Menu, app }, { menu, label, pattern }) => {
        const plain = (text: string) => text.replace(/&/g, "");
        const matches = (text: string) =>
          pattern !== null
            ? new RegExp(pattern.source, pattern.flags).test(plain(text))
            : plain(text).toLowerCase() === label?.toLowerCase();
        const top = Menu.getApplicationMenu()?.items.find((i) => plain(i.label) === menu);
        const entry = top?.submenu?.items.find((i) => matches(i.label));
        if (entry === undefined) return false;
        if (entry.role === "quit" || entry.role === "close") {
          // A role item has no click handler of ours: Exit is `app.quit()`.
          app.quit();
          return true;
        }
        entry.click();
        return true;
      },
      { menu, label, pattern },
    );
    if (!found) throw new Error(`no menu item ${menu} > ${String(item)}`);
    return;
  }
  await page
    .getByRole("menubar", { name: "Application menu" })
    .getByRole("menuitem", { name: menu, exact: true })
    .click();
  const list = page.getByRole("menu", { name: menu });
  await expect(list).toBeVisible();
  const name =
    typeof item === "string"
      ? new RegExp(`^${item.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(?:\\s+\\S+)?$`)
      : item;
  await list.getByRole("menuitem", { name }).click();
  // File > Exit can close the window before the menu is seen to close.
  await expect(list)
    .toBeHidden()
    .catch((error: unknown) => {
      if (!page.isClosed()) throw error;
    });
}

// ---- the OS file pickers ---------------------------------------------------------------------

declare global {
  /** Main-process state of the stubbed file pickers (installed by `pickFiles`). */
  var __cutweaveFlowPicker:
    | { answers: (string[] | null)[]; asked: { properties: string[] }[] }
    | undefined;
}

/**
 * The next native file pickers answer with these paths, in order (`null`: the person cancels).
 * Stubs `dialog.showOpenDialog` in main: the OS dialog the app does not own. Returns a reader
 * of what the app asked the OS for (the picker's `properties`).
 */
export async function pickFiles(
  app: ElectronApplication,
  ...answers: (string[] | null)[]
): Promise<() => Promise<{ properties: string[] }[]>> {
  await app.evaluate(({ dialog }, answers) => {
    globalThis.__cutweaveFlowPicker ??= { answers: [], asked: [] };
    const state = globalThis.__cutweaveFlowPicker;
    state.answers.push(...answers);
    dialog.showOpenDialog = (async (...args: unknown[]) => {
      const options = (args.length > 1 ? args[1] : args[0]) as { properties?: string[] };
      state.asked.push({ properties: [...(options?.properties ?? [])] });
      const next = state.answers.shift() ?? null;
      return next === null
        ? { canceled: true, filePaths: [] }
        : { canceled: false, filePaths: next };
    }) as typeof dialog.showOpenDialog;
  }, answers);
  return () => app.evaluate(() => globalThis.__cutweaveFlowPicker?.asked ?? []);
}

declare global {
  /** Main-process state of the stubbed save dialog (installed by `pickSavePath`). */
  var __cutweaveFlowSave:
    | { answers: (string | null)[]; asked: { defaultPath: string | undefined }[] }
    | undefined;
}

/**
 * The next native "save as" dialogs answer with these paths, in order (`null`: the person
 * cancels). Stubs `dialog.showSaveDialog` in main: the OS dialog the app does not own. Returns a
 * reader of the default path the app proposed each time.
 */
export async function pickSavePath(
  app: ElectronApplication,
  ...answers: (string | null)[]
): Promise<() => Promise<(string | undefined)[]>> {
  await app.evaluate(({ dialog }, answers) => {
    globalThis.__cutweaveFlowSave ??= { answers: [], asked: [] };
    const state = globalThis.__cutweaveFlowSave;
    state.answers.push(...answers);
    dialog.showSaveDialog = (async (...args: unknown[]) => {
      const options = (args.length > 1 ? args[1] : args[0]) as { defaultPath?: string };
      state.asked.push({ defaultPath: options?.defaultPath });
      const next = state.answers.shift() ?? null;
      return next === null ? { canceled: true, filePath: "" } : { canceled: false, filePath: next };
    }) as typeof dialog.showSaveDialog;
  }, answers);
  return () =>
    app.evaluate(() => (globalThis.__cutweaveFlowSave?.asked ?? []).map((a) => a.defaultPath));
}

// ---- reading the document back -------------------------------------------------------------

export interface ItemView {
  id: string;
  start: number;
  duration: number;
  media: string;
  linked_group: string | null;
  enabled: boolean;
}
export interface TrackView {
  id: string;
  kind: "video" | "audio";
  name: string;
  items: ItemView[];
}
export interface SequenceView {
  id: string;
  tracks: TrackView[];
}

/** The project's first sequence with its tracks (top to bottom) and items, from the engine. */
export async function sequence(page: Page): Promise<SequenceView> {
  const info = await editorCall<{ sequences: { id: string }[] }>(page, "project.info");
  const id = info.sequences[0]?.id;
  if (id === undefined) throw new Error("the project has no sequence");
  const seq = await editorCall<SequenceView>(page, "timeline.get", { sequence: id, depth: 2 });
  for (const track of seq.tracks) track.items.sort((a, b) => a.start - b.start);
  return seq;
}

/** Item starts and durations per track, in seconds (ids left out): what the person sees. */
export async function layout(page: Page): Promise<{ track: string; clips: [number, number][] }[]> {
  const seq = await sequence(page);
  return seq.tracks.map((t) => ({
    track: t.name,
    clips: t.items.map((i) => [i.start / FLICKS, i.duration / FLICKS] as [number, number]),
  }));
}

const baseName = (p: string | null): string => (p ?? "").split(/[\\/]/).pop() ?? "";

/** The project's media (`media.list`): id and file name. */
async function mediaList(page: Page): Promise<{ id: string; name: string }[]> {
  const out = await editorCall<{ media: { id: string; path: string | null }[] }>(
    page,
    "media.list",
    {},
  );
  return out.media.map((m) => ({ id: m.id, name: baseName(m.path) }));
}

/** The project's media, by file name. */
export async function mediaNames(page: Page): Promise<string[]> {
  return (await mediaList(page)).map((m) => m.name).sort();
}

/** Media names of the clips on a track, in timeline order. */
export async function clipNames(page: Page, trackName: string): Promise<string[]> {
  const [seq, media] = await Promise.all([sequence(page), mediaList(page)]);
  const names = new Map(media.map((m) => [m.id, m.name]));
  const track = seq.tracks.find((t) => t.name === trackName);
  return (track?.items ?? []).map((i) => names.get(i.media) ?? i.media);
}

/**
 * Waits until the editor's own document has caught up with the engine (the timeline draws from
 * it): the item count the engine reports is what the editor holds.
 */
export async function editorCaughtUp(page: Page): Promise<void> {
  await expect
    .poll(async () => {
      const seq = await sequence(page).catch(() => null);
      const engineItems = seq?.tracks.reduce((n, t) => n + t.items.length, 0) ?? -1;
      return (await editorState(page)).items === engineItems;
    })
    .toBe(true);
}

// ---- the timeline canvas ---------------------------------------------------------------------

/**
 * Where things are drawn on the timeline canvas, from the same tokens and view state the
 * renderer uses (`@cutweave/timeline` TimelineView/TimelineLayout): header `4 × space-12` wide,
 * ruler `control-md` tall, each lane `track-md + 2 × space-1` tall.
 */
export interface TimelineGeometry {
  readonly box: { x: number; y: number; width: number; height: number };
  readonly headerWidth: number;
  readonly rulerHeight: number;
  readonly laneHeight: number;
  readonly pxPerSec: number;
  readonly scrollX: number;
  readonly scrollY: number;
  readonly tracks: readonly TrackView[];
}

/**
 * `drawnIn` is the window the Timeline is shown in, when it is detached into its own (WP-2H.15);
 * the engine's view and sequence are still asked of the editor `page`.
 */
export async function timelineGeometry(
  page: Page,
  drawnIn: Page = page,
): Promise<TimelineGeometry> {
  const box = await timelineCanvas(drawnIn).boundingBox();
  if (box === null) throw new Error("the timeline canvas is not visible");
  const tokens = await drawnIn.evaluate(() => {
    const cs = getComputedStyle(document.documentElement);
    const px = (name: string) => Number.parseFloat(cs.getPropertyValue(name));
    return {
      space12: px("--space-12"),
      controlMd: px("--control-md"),
      trackMd: px("--track-md"),
      space1: px("--space-1"),
    };
  });
  const view = await editorCall<{ zoom: number | null; scroll: { x: number; y: number } }>(
    page,
    "view.get",
  );
  const seq = await sequence(page);
  return {
    box,
    headerWidth: 4 * tokens.space12,
    rulerHeight: tokens.controlMd,
    laneHeight: tokens.trackMd + 2 * tokens.space1,
    pxPerSec: view.zoom ?? 100,
    scrollX: view.scroll.x,
    scrollY: view.scroll.y,
    tracks: seq.tracks,
  };
}

/** Page x of timeline time `t` (flicks). */
export const xAt = (g: TimelineGeometry, t: number): number =>
  g.box.x + g.headerWidth + ((t - g.scrollX) * g.pxPerSec) / FLICKS;

/** Page y of the middle of a track's lane. */
export function laneY(g: TimelineGeometry, trackName: string): number {
  const index = g.tracks.findIndex((t) => t.name === trackName);
  if (index < 0) throw new Error(`no track ${trackName}`);
  return g.box.y + g.rulerHeight + index * g.laneHeight - g.scrollY + g.laneHeight / 2;
}

/** Page y of the ruler. */
export const rulerY = (g: TimelineGeometry): number => g.box.y + g.rulerHeight / 2;

/** The page rectangle of a clip. */
export function clipBox(g: TimelineGeometry, itemId: string) {
  for (const track of g.tracks) {
    const item = track.items.find((i) => i.id === itemId);
    if (item === undefined) continue;
    const x = xAt(g, item.start);
    const w = (item.duration * g.pxPerSec) / FLICKS;
    return { x, w, y: laneY(g, track.name), track: track.name, item };
  }
  throw new Error(`no item ${itemId} on the timeline`);
}

/** The clip on `trackName` that starts at `startSec` (to the frame). */
export function clipAt(g: TimelineGeometry, trackName: string, startSec: number): ItemView {
  const track = g.tracks.find((t) => t.name === trackName);
  const item = track?.items.find((i) => Math.abs(i.start - sec(startSec)) < FRAME / 2);
  if (item === undefined) throw new Error(`no clip at ${startSec} s on ${trackName}`);
  return item;
}

/** A pointer drag with the left button, in small steps like a hand moving the mouse. */
export async function drag(
  page: Page,
  from: { x: number; y: number },
  to: { x: number; y: number },
): Promise<void> {
  await page.mouse.move(from.x, from.y);
  await page.mouse.down();
  await page.mouse.move(from.x + Math.sign(to.x - from.x) * 4, from.y, { steps: 2 });
  await page.mouse.move(to.x, to.y, { steps: 12 });
  await page.mouse.up();
}

/** Drags a media card from the bin onto a track at `atSec` (HTML drag and drop, as a hand does). */
export async function dropCard(
  page: Page,
  name: string,
  trackName: string,
  atSec: number,
  offsetPx = 0,
): Promise<void> {
  const g = await timelineGeometry(page);
  await mediaCard(page, name).dragTo(timelineCanvas(page), {
    targetPosition: {
      x: xAt(g, sec(atSec)) + offsetPx - g.box.x,
      y: laneY(g, trackName) - g.box.y,
    },
  });
}

/**
 * Clicks the timeline toolbar's Zoom out (WP-2A.10) until the first `seconds` of the timeline
 * show across the lanes, as a person does before dropping or dragging far to the right.
 */
export async function showTimeline(page: Page, seconds: number): Promise<void> {
  const zoomOut = timelineTools(page).getByRole("button", { name: "Zoom out", exact: true });
  for (let n = 0; n < 12; n += 1) {
    const g = await timelineGeometry(page);
    if (xAt(g, sec(seconds)) < g.box.x + g.box.width - 8) return;
    await zoomOut.click();
    await expect.poll(async () => (await timelineGeometry(page)).pxPerSec).toBeLessThan(g.pxPerSec);
  }
  throw new Error(`the timeline never showed ${seconds} s`);
}

// ---- project actions -------------------------------------------------------------------------

/** The "New project" / "Save project as" dialog. */
export function namingDialog(page: Page, title: "New project" | "Save project as" = "New project") {
  const dialog = page.getByRole("dialog", { name: title });
  return {
    dialog,
    name: dialog.getByLabel("Name"),
    location: dialog.getByLabel("Location"),
    choose: dialog.getByRole("button", { name: "Choose folder…" }),
    create: dialog.getByRole("button", {
      name: title === "New project" ? "Create project" : "Save project",
    }),
    cancel: dialog.getByRole("button", { name: "Cancel" }),
  };
}

/** The project's `.cutweave` document for a project folder `<parent>/<name>`. */
export const projectDoc = (parent: string, name: string): string =>
  path.join(parent, name, `${name}.cutweave`);

/**
 * File > New Project (Ctrl+N): types the name and the location into the dialog and creates it.
 * Resolves once the window title names it and nothing is unsaved.
 */
export async function createProject(
  app: ElectronApplication,
  page: Page,
  name: string,
  parent: string,
): Promise<string> {
  await shortcut(app, "Control+N");
  const naming = namingDialog(page);
  await expect(naming.dialog).toBeVisible();
  await naming.name.fill(name);
  await naming.location.fill(parent);
  await naming.create.click();
  await expect(naming.dialog).toBeHidden();
  await expect(page).toHaveTitle(`${name} — Cutweave`);
  await expect(dirtyDot(page)).toBeHidden();
  await expect(timelineCanvas(page)).toBeVisible();
  return projectDoc(parent, name);
}

/**
 * Imports files with the media bin's Import button (the OS picker returns `files`). Resolves
 * once the bin shows them and the editor holds them.
 */
export async function importWithButton(
  app: ElectronApplication,
  page: Page,
  files: readonly string[],
): Promise<void> {
  const before = await mediaCards(page).count();
  await pickFiles(app, [...files]);
  await page.getByRole("button", { name: "Import", exact: true }).click();
  await expect(mediaCards(page)).toHaveCount(before + files.length);
}

/**
 * Opens File > Open recent and returns its list (macOS: the top bar's Recent menu, since the
 * native menu has no Open recent). Escape closes it.
 */
export async function showRecent(page: Page): Promise<Locator> {
  if (process.platform === "darwin") {
    await toolbar(page).getByRole("button", { name: "Recent projects" }).click();
    return page.getByRole("menu", { name: "Recent projects" });
  }
  await page
    .getByRole("menubar", { name: "Application menu" })
    .getByRole("menuitem", { name: "File", exact: true })
    .click();
  await page
    .getByRole("menu", { name: "File" })
    .getByRole("menuitem", { name: /^Open recent/ })
    .click();
  const recent = page.getByRole("menu", { name: "Open recent" });
  await expect(recent).toBeVisible();
  return recent;
}

/** File > Open recent > the project with this file name. */
export async function openRecent(page: Page, fileName: string): Promise<void> {
  const recent = await showRecent(page);
  await recent.getByRole("menuitem", { name: fileName }).click();
  await expect(recent).toBeHidden();
}

/** File > Settings… (Ctrl+,; ⌘, in the macOS app menu): the open dialog. */
export async function openSettings(page: Page, app: ElectronApplication): Promise<Locator> {
  if (process.platform === "darwin") await shortcut(app, "Control+,");
  else await chooseMenu(page, app, "File", "Settings…");
  const settings = page.getByRole("dialog", { name: "Settings" });
  await expect(settings).toBeVisible();
  return settings;
}

/** File > Settings…, then the page with this name in the dialog's page list. */
export async function openSettingsPage(
  page: Page,
  app: ElectronApplication,
  name:
    | "General"
    | "Appearance"
    | "Editing"
    | "Media and import"
    | "Export"
    | "Saving and backups"
    | "Keyboard shortcuts"
    | "Notifications"
    | "Agents and MCP"
    | "Cache and storage"
    | "Performance and hardware"
    | "Advanced",
): Promise<Locator> {
  const settings = await openSettings(page, app);
  await settings.getByRole("tab", { name }).click();
  await expect(settings.getByRole("heading", { name, level: 3 })).toBeVisible();
  return settings;
}

/** Closes Settings with Done. */
export async function closeSettings(settings: Locator): Promise<void> {
  await settings.getByRole("button", { name: "Done" }).click();
  await expect(settings).toBeHidden();
}

/**
 * File > Settings… > Appearance, picks the theme and closes with Done. The theme lives in
 * Settings, not in the title bar.
 */
export async function chooseTheme(
  page: Page,
  app: ElectronApplication,
  theme: "Dark" | "Light",
): Promise<void> {
  const settings = await openSettingsPage(page, app, "Appearance");
  const radio = settings.getByRole("radiogroup", { name: "Theme" }).getByRole("radio", {
    name: theme,
  });
  await radio.click();
  await expect(radio).toHaveAttribute("aria-checked", "true");
  await closeSettings(settings);
}

/** The title bar's save status (WP-1M.10): its words are its tooltip and its text for readers. */
export const saveStatus = (page: Page): Locator => page.getByTestId("save-status");

/** The crash-recovery dialog (WP-1M.07). */
export function recoveryDialog(page: Page) {
  const dialog = page.getByRole("dialog", { name: "Restore unsaved changes?" });
  return {
    dialog,
    restore: dialog.getByRole("button", { name: /^Restore unsaved changes/ }),
    discard: dialog.getByRole("button", { name: "Discard", exact: true }),
  };
}

/**
 * Resolves once no process of an app killed on profile `home` holds Chromium's single-instance
 * lock (`<userData>/lockfile`, opened exclusively and deleted when its holder exits). The killed
 * app's last helper process keeps it for a moment after the main process has gone, and a
 * relaunch that races it quits at once (issue #147).
 */
export async function waitProfileFree(home: string): Promise<void> {
  const lock = path.join(home, "userData", "lockfile");
  const free = (): boolean => {
    if (!existsSync(lock)) return true;
    try {
      closeSync(openSync(lock, "r+"));
      return true;
    } catch {
      return false;
    }
  };
  await expect.poll(free, { timeout: 30_000, message: "the profile stays locked" }).toBe(true);
}

/** Whether main knows the editor has unsaved changes (read-only test hook). */
export async function windowDirty(app: ElectronApplication): Promise<boolean> {
  return app.evaluate(() => globalThis.__cutweaveTest?.windowDirty() ?? false);
}

/** Selects a clip by clicking its body on the timeline; waits for the inspector to show it. */
export async function clickClip(page: Page, itemId: string): Promise<void> {
  const g = await timelineGeometry(page);
  const box = clipBox(g, itemId);
  await page.mouse.click(box.x + Math.min(box.w / 2, 60), box.y);
  await expect(page.getByTestId("clip-inspector")).toBeVisible();
}

/** The inspector's fields for the selected clip. */
export function inspector(page: Page) {
  const panel = page.getByTestId("clip-inspector");
  return {
    panel,
    name: panel.getByLabel("Name", { exact: true }),
    start: panel.getByLabel("Start"),
    duration: panel.getByLabel("Duration"),
    opacity: panel.getByRole("slider", { name: "Opacity" }),
    positionX: panel.getByRole("spinbutton", { name: "Position X" }),
    positionY: panel.getByRole("spinbutton", { name: "Position Y" }),
    scale: panel.getByRole("spinbutton", { name: "Scale X" }),
    rotation: panel.getByRole("spinbutton", { name: "Rotation" }),
  };
}

/** The item as the engine has it (`item.get`). */
export async function item(page: Page, id: string) {
  return editorCall<{
    item: { id: string; start: number; duration: number };
    name: string | null;
    linked: string[];
    visual: {
      opacity: number;
      transform: { position: { x: number; y: number }; scale: { x: number }; rotation: number };
    };
    audio: { gain: number; pan: number; fade_in?: { duration: number } };
    /** Where it reads its media, with speed and direction (WP-2D.01). */
    source?: { source_in: number; speed: { num: number; den: number }; reverse: boolean };
  }>(page, "item.get", { item: id });
}

/** The first sequence's format as the engine has it (`project.info`). */
export async function sequenceFormat(page: Page) {
  const info = await editorCall<{
    sequences: {
      format: {
        width: number;
        height: number;
        frame_rate: { num: number; den: number };
        background: string;
      };
    }[];
  }>(page, "project.info");
  const first = info.sequences[0];
  if (first === undefined) throw new Error("the project has no sequence");
  return first.format;
}

/** An item's position as the engine stores it: static, or keyed (item time in flicks). */
export async function positionOf(
  page: Page,
  id: string,
): Promise<{
  x?: number;
  y?: number;
  keyframes?: { t: number; value: { x: number; y: number } }[];
}> {
  const got = await editorCall<{
    visual: {
      transform: {
        position: {
          x?: number;
          y?: number;
          keyframes?: { t: number; value: { x: number; y: number } }[];
        };
      };
    };
  }>(page, "item.get", { item: id });
  return got.visual.transform.position;
}
