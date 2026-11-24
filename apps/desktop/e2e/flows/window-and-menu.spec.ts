// SPDX-License-Identifier: GPL-3.0-or-later
// F18 Window and menu (docs/user-flows.md, WP-1M.09): the custom title bar and the in-app menu
// bar, driven like a person drives them (ADR-0018).
//
// - Every item of File, Edit, View and Help is clicked in the menu bar, and its shortcut is
//   pressed; the flow checks what the person sees (title, dialogs, the unsaved dot, the timeline,
//   the field they type in) and gets (project folders on disk, the clipboard, full screen).
// - File > Exit quits through the unsaved-changes dialog, answered in the UI.
// - The title bar: no native menu, the OS window controls drawn in the overlay the layout keeps
//   free, in the theme's colors, and dragging and double-clicking the title area move and
//   maximize the window (real OS mouse input, `os-input.ts`; Windows).
//
// Only the OS folder/file pickers are stubbed (they return the path a person would pick). Main's
// state is read back through `app.evaluate` (read-only); nothing changes how the app behaves.
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import type { ElectronApplication, Locator, Page } from "@playwright/test";
import { colors } from "../../../../packages/tokens/src/generated/tokens.ts";
import {
  editorHandle,
  editorState,
  expect,
  hasRealEngine,
  type LaunchedApp,
  launchApp,
  REAL_ENGINE_ENV,
  test,
  unsavedPrompt,
} from "../fixtures.ts";
import { chooseMenu, chooseTheme } from "./helpers.ts";
import {
  osClick,
  osDoubleClick,
  osDrag,
  osInputAvailable,
  osPixel,
  ownerOf,
  type Point,
  screenPoint,
} from "./os-input.ts";

// ---- setup -------------------------------------------------------------------------------------

/** A throwaway folder: `media/clip.mp4` (a stand-in the stub prober reads by extension). */
function workspace() {
  const dir = mkdtempSync(path.join(tmpdir(), "cutweave-menu-"));
  const media = path.join(dir, "media");
  const projects = path.join(dir, "projects");
  mkdirSync(media);
  mkdirSync(projects);
  const clip = path.join(media, "clip.mp4");
  writeFileSync(clip, "placeholder for clip.mp4 (Cutweave e2e)\n");
  return {
    dir,
    clip,
    projects,
    remove: () => rmSync(dir, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 }),
  };
}

/** The built app with the real engine, stub media probing and the workspace as an engine root. */
async function launchFlow(dir: string): Promise<LaunchedApp> {
  const launched = await launchApp({
    ...REAL_ENGINE_ENV,
    CUTWEAVE_DEV_STUB_MEDIA: "1",
    CUTWEAVE_ENGINE_ALLOW_ROOTS: dir,
  });
  await editorHandle(launched.page);
  await expect.poll(async () => (await editorState(launched.page)).connection).toBe("ready");
  await expect(launched.page.locator(".cw-engine-status")).toHaveText("Engine connected");
  return launched;
}

/**
 * The next OS file/folder pickers answer with these paths, in order (`null`: cancelled). Stubs
 * `dialog.showOpenDialog` in main: the OS dialog the app does not own.
 */
async function pickPaths(app: ElectronApplication, ...answers: (string[] | null)[]) {
  await app.evaluate(({ dialog }, queued) => {
    const state = globalThis as unknown as { __cwPicks?: (string[] | null)[] };
    state.__cwPicks = [...(state.__cwPicks ?? []), ...queued];
    dialog.showOpenDialog = (async () => {
      const next = state.__cwPicks?.shift() ?? null;
      return next === null
        ? { canceled: true, filePaths: [] }
        : { canceled: false, filePaths: next };
    }) as typeof dialog.showOpenDialog;
  }, answers);
}

// ---- what the person sees ----------------------------------------------------------------------

const menuBar = (page: Page): Locator => page.getByRole("menubar", { name: "Application menu" });
const menuTrigger = (page: Page, menu: string): Locator =>
  menuBar(page).getByRole("menuitem", { name: menu, exact: true });
const dirtyDot = (page: Page): Locator => page.getByTestId("dirty-dot");
const anyDialog = (page: Page): Locator => page.getByRole("dialog");

const escapeRe = (s: string): string => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** A command in an open menu by its label (its name ends with the shortcut, if it has one). */
function item(scope: Locator, label: string | RegExp): Locator {
  const name = typeof label === "string" ? new RegExp(`^${escapeRe(label)}(?:\\s+\\S+)?$`) : label;
  return scope.getByRole("menuitem", { name });
}

/** Opens a menu of the bar with a click and returns its list. */
async function openMenu(page: Page, menu: string): Promise<Locator> {
  await menuTrigger(page, menu).click();
  const list = page.getByRole("menu", { name: menu });
  await expect(list).toBeVisible();
  return list;
}

/** The naming dialog of New project / Save as. */
function namingDialog(page: Page, title: "New project" | "Save project as") {
  const dialog = page.getByRole("dialog", { name: title });
  return {
    dialog,
    name: dialog.getByLabel("Name"),
    location: dialog.getByLabel("Location"),
    choose: dialog.getByRole("button", { name: "Choose folder…" }),
    confirm: dialog.getByRole("button", {
      name: title === "New project" ? "Create project" : "Save project",
    }),
  };
}

/** Names the project and its place in the open naming dialog, confirms, waits for the title. */
async function nameProject(
  page: Page,
  title: "New project" | "Save project as",
  name: string,
  parent: string,
): Promise<string> {
  const naming = namingDialog(page, title);
  await expect(naming.dialog).toBeVisible();
  await naming.name.fill(name);
  await naming.location.fill(parent);
  await naming.confirm.click();
  await expect(naming.dialog).toBeHidden();
  // A new project is titled by its name; Save as keeps the project's name in a new folder.
  if (title === "New project") await expect(page).toHaveTitle(`${name} — Cutweave`);
  await projectActionsReady(page);
  return path.join(parent, name, `${name}.cutweave`);
}

/**
 * Waits until no project action is running, as a person sees it: File > New project… is enabled
 * again (the menu's project commands, and their shortcuts, are disabled while one runs). Opens
 * the File menu and closes it with Escape.
 */
async function projectActionsReady(page: Page): Promise<void> {
  const bar = page.getByRole("menubar", { name: "Application menu" });
  await bar.getByRole("menuitem", { name: "File", exact: true }).click();
  const menu = page.getByRole("menu", { name: "File" });
  await expect(menu.getByRole("menuitem", { name: /^New project/ })).toBeEnabled();
  await page.keyboard.press("Escape");
  await expect(menu).toBeHidden();
}

/** Items and media in a saved project document. */
function onDisk(doc: string): { items: number; media: number } {
  const saved = JSON.parse(readFileSync(doc, "utf8")) as {
    media: Record<string, unknown>;
    sequences: Record<string, { tracks: Record<string, { items: Record<string, unknown> }> }>;
  };
  let items = 0;
  for (const seq of Object.values(saved.sequences)) {
    for (const track of Object.values(seq.tracks)) items += Object.keys(track.items).length;
  }
  return { items, media: Object.keys(saved.media).length };
}

/** Clips on the timeline, as the editor holds them. */
async function timelineItems(page: Page): Promise<number> {
  return (await editorState(page)).items;
}

/** Imports `file` with the media bin's Import button (the OS picker returns it). */
async function importClip(app: ElectronApplication, page: Page, file: string): Promise<Locator> {
  await pickPaths(app, [file]);
  await page.getByRole("button", { name: "Import", exact: true }).click();
  const card = page
    .getByRole("listbox", { name: "Media files" })
    .getByRole("option")
    .filter({ hasText: path.basename(file) });
  await expect(card).toHaveCount(1);
  return card;
}

/** The OS clipboard's text (read-only). */
const clipboardText = (app: ElectronApplication): Promise<string> =>
  app.evaluate(({ clipboard }) => clipboard.readText());

/** Puts other text on the OS clipboard, as copying something elsewhere would. */
const putOnClipboard = (app: ElectronApplication, text: string): Promise<void> =>
  app.evaluate(({ clipboard }, t) => clipboard.writeText(t), text);

/** The focused field's text and selection. */
function focusedField(page: Page) {
  return page.evaluate(() => {
    const el = document.activeElement as HTMLInputElement | null;
    return {
      tag: el?.tagName ?? null,
      value: el?.value ?? null,
      start: el?.selectionStart ?? null,
      end: el?.selectionEnd ?? null,
    };
  });
}

/**
 * Keeps the app's window in front of the desktop for a real-mouse step (other apps may open
 * windows meanwhile): a window-manager setting, the app itself is not changed.
 */
function toFront(app: ElectronApplication): Promise<void> {
  return app.evaluate(({ BrowserWindow }) => {
    const win = BrowserWindow.getAllWindows()[0];
    win?.setAlwaysOnTop(true, "screen-saver");
    win?.moveTop();
  });
}

/** Main's view of the editor window (read-only). */
function windowState(app: ElectronApplication) {
  return app.evaluate(({ BrowserWindow }) => {
    const win = BrowserWindow.getAllWindows()[0];
    if (win === undefined) throw new Error("no window");
    return {
      bounds: win.getBounds(),
      maximized: win.isMaximized(),
      focused: win.isFocused(),
      fullScreen: win.isFullScreen(),
      devTools: win.webContents.isDevToolsOpened(),
    };
  });
}

// ---- the flows ---------------------------------------------------------------------------------

test.describe("F18 window and menu", () => {
  test.skip(!hasRealEngine, "cutweave-engine-serve is not built");
  let work: ReturnType<typeof workspace>;
  test.beforeEach(() => {
    work = workspace();
  });
  test.afterEach(() => {
    work.remove();
  });

  test("File: new, open, open recent, save, save as, export, by click and by shortcut", async () => {
    test.setTimeout(150_000);
    const { app, page, close } = await launchFlow(work.dir);
    try {
      // New project… by click; the place comes from the OS folder picker.
      await chooseMenu(page, app, "File", "New project…");
      const alphaNaming = namingDialog(page, "New project");
      await alphaNaming.name.fill("Alpha");
      await pickPaths(app, [work.projects]);
      await alphaNaming.choose.click();
      await expect(alphaNaming.location).toHaveValue(work.projects);
      await alphaNaming.confirm.click();
      await expect(page).toHaveTitle("Alpha — Cutweave");
      await projectActionsReady(page);
      const alpha = path.join(work.projects, "Alpha", "Alpha.cutweave");
      expect(onDisk(alpha)).toEqual({ items: 0, media: 0 });
      await expect(dirtyDot(page)).toBeHidden();

      // New project… by Ctrl+N.
      await page.keyboard.press("Control+N");
      const beta = await nameProject(page, "New project", "Beta", work.projects);
      expect(onDisk(beta).items).toBe(0);

      // Open project… by click and by Ctrl+O (the OS picker returns the folder).
      await pickPaths(app, [path.dirname(alpha)]);
      await chooseMenu(page, app, "File", "Open project…");
      await expect(page).toHaveTitle("Alpha — Cutweave");
      await projectActionsReady(page);
      await pickPaths(app, [path.dirname(beta)]);
      await page.keyboard.press("Control+O");
      await expect(page).toHaveTitle("Beta — Cutweave");
      await projectActionsReady(page);

      // Open recent > Alpha.
      const file = await openMenu(page, "File");
      await item(file, "Open recent").click();
      const recent = page.getByRole("menu", { name: "Open recent" });
      await expect(recent).toBeVisible();
      await expect(recent.getByRole("menuitem")).toHaveCount(2);
      await recent.getByRole("menuitem", { name: /^Alpha/ }).click();
      await expect(recent).toBeHidden();
      await expect(page).toHaveTitle("Alpha — Cutweave");
      await projectActionsReady(page);

      // An edit: import a clip and insert it with Enter.
      const card = await importClip(app, page, work.clip);
      await card.click();
      await page.keyboard.press("Enter");
      await expect.poll(() => timelineItems(page)).toBeGreaterThan(0);
      const inserted = await timelineItems(page);
      await expect(dirtyDot(page)).toBeVisible();

      // Save by click: the dot goes and the clip is in the document on disk.
      await chooseMenu(page, app, "File", "Save");
      await expect(dirtyDot(page)).toBeHidden();
      await projectActionsReady(page);
      expect(onDisk(alpha)).toEqual({ items: inserted, media: 1 });

      // Save by Ctrl+S after one more change (undo of the insert, from the toolbar).
      await page.getByRole("button", { name: /^Undo/ }).click();
      await expect.poll(() => timelineItems(page)).toBe(0);
      await expect(dirtyDot(page)).toBeVisible();
      await page.keyboard.press("Control+S");
      await expect(dirtyDot(page)).toBeHidden();
      await projectActionsReady(page);
      expect(onDisk(alpha).items).toBe(0);

      // Save as… by click, then by Ctrl+Shift+S: the project continues in the new folder,
      // which Open recent now lists first.
      await chooseMenu(page, app, "File", "Save as…");
      const gamma = await nameProject(page, "Save project as", "Gamma", work.projects);
      expect(onDisk(gamma).media).toBe(1);
      await page.keyboard.press("Control+Shift+S");
      const delta = await nameProject(page, "Save project as", "Delta", work.projects);
      expect(onDisk(delta).media).toBe(1);
      await expect(page).toHaveTitle("Alpha — Cutweave");
      await expect(dirtyDot(page)).toBeHidden();
      const files = await openMenu(page, "File");
      await item(files, "Open recent").click();
      const listed = page.getByRole("menu", { name: "Open recent" }).getByRole("menuitem");
      await expect(listed).toHaveText([
        "Delta.cutweave",
        "Gamma.cutweave",
        "Alpha.cutweave",
        "Beta.cutweave",
      ]);
      await page.keyboard.press("Escape");
      await page.keyboard.press("Escape");
      await expect(page.getByRole("menu")).toHaveCount(0);

      // Export… is listed with its shortcut but waits for the export dialog (WP-1M.08).
      const fileAgain = await openMenu(page, "File");
      const exportItem = item(fileAgain, "Export…");
      await expect(exportItem).toHaveAttribute("aria-disabled", "true");
      await expect(exportItem).toContainText("Ctrl+M");
      await page.keyboard.press("Escape");
      await expect(fileAgain).toBeHidden();
      await page.keyboard.press("Control+M");
      await expect(anyDialog(page)).toHaveCount(0);
    } finally {
      await close();
    }
  });

  test("Edit: undo, redo, cut, copy, paste and select all, by click and by shortcut", async () => {
    test.setTimeout(120_000);
    const { app, page, close } = await launchFlow(work.dir);
    try {
      await page.keyboard.press("Control+N");
      await nameProject(page, "New project", "Edits", work.projects);
      const card = await importClip(app, page, work.clip);
      await card.click();
      await page.keyboard.press("Enter");
      // The clip and, for a video with sound, its linked audio.
      await expect.poll(() => timelineItems(page)).toBeGreaterThan(0);
      const inserted = await timelineItems(page);

      // Undo and Redo from the menu (named after the step), then Ctrl+Z, Ctrl+Y, Ctrl+Shift+Z.
      const edit = await openMenu(page, "Edit");
      await expect(item(edit, /^Redo/)).toHaveAttribute("aria-disabled", "true");
      await item(edit, /^Undo \S/).click();
      await expect.poll(() => timelineItems(page)).toBe(0);
      await chooseMenu(page, app, "Edit", /^Redo \S/);
      await expect.poll(() => timelineItems(page)).toBe(inserted);
      await page.keyboard.press("Control+Z");
      await expect.poll(() => timelineItems(page)).toBe(0);
      await page.keyboard.press("Control+Y");
      await expect.poll(() => timelineItems(page)).toBe(inserted);
      await page.keyboard.press("Control+Z");
      await expect.poll(() => timelineItems(page)).toBe(0);
      await page.keyboard.press("Control+Shift+Z");
      await expect.poll(() => timelineItems(page)).toBe(inserted);

      // Insert it once more: the new clip is selected, and its name is the text to edit.
      await card.click();
      await page.keyboard.press("Enter");
      await expect.poll(() => timelineItems(page)).toBe(2 * inserted);
      const nameField = page.getByTestId("clip-inspector").getByLabel("Name");
      await expect(nameField).toBeVisible();
      const name = "Opening shot";
      await nameField.click();
      await page.keyboard.type(name);
      await page.keyboard.press("Enter");
      await expect(nameField).toHaveValue(name);

      // Select all, Copy, Paste and Cut from the menu act on the field that had the focus.
      await chooseMenu(page, app, "Edit", "Select all");
      await expect
        .poll(() => focusedField(page))
        .toEqual({ tag: "INPUT", value: name, start: 0, end: name.length });
      await putOnClipboard(app, "something else");
      await chooseMenu(page, app, "Edit", "Copy");
      await expect.poll(() => clipboardText(app)).toBe(name);
      await page.keyboard.press("End");
      await chooseMenu(page, app, "Edit", "Paste");
      await expect(nameField).toHaveValue(`${name}${name}`);
      // Escape puts the clip's name back without renaming it.
      await page.keyboard.press("Escape");
      await expect(nameField).toHaveValue(name);
      await chooseMenu(page, app, "Edit", "Select all");
      await putOnClipboard(app, "something else");
      await chooseMenu(page, app, "Edit", "Cut");
      await expect(nameField).toHaveValue("");
      await expect.poll(() => clipboardText(app)).toBe(name);
      await page.keyboard.press("Escape");
      await expect(nameField).toHaveValue(name);

      // The same with Ctrl+A, Ctrl+C, Ctrl+V and Ctrl+X in the field.
      await page.keyboard.press("Control+A");
      await expect
        .poll(() => focusedField(page))
        .toEqual({ tag: "INPUT", value: name, start: 0, end: name.length });
      await putOnClipboard(app, "something else");
      await page.keyboard.press("Control+C");
      await expect.poll(() => clipboardText(app)).toBe(name);
      await page.keyboard.press("End");
      await page.keyboard.press("Control+V");
      await expect(nameField).toHaveValue(`${name}${name}`);
      await page.keyboard.press("Escape");
      await page.keyboard.press("Control+A");
      await putOnClipboard(app, "something else");
      await page.keyboard.press("Control+X");
      await expect(nameField).toHaveValue("");
      await expect.poll(() => clipboardText(app)).toBe(name);
      await page.keyboard.press("Escape");
      await expect(nameField).toHaveValue(name);

      // Ctrl+Z while typing undoes the typing, not the project.
      const undoName = await page.getByRole("button", { name: /^Undo/ }).getAttribute("aria-label");
      await page.keyboard.press("End");
      await page.keyboard.type("!");
      await expect(nameField).toHaveValue(`${name}!`);
      await page.keyboard.press("Control+Z");
      await expect(nameField).toHaveValue(name);
      expect(await timelineItems(page)).toBe(2 * inserted);
      await expect(page.getByRole("button", { name: /^Undo/ })).toHaveAttribute(
        "aria-label",
        undoName ?? "",
      );
      await page.keyboard.press("Escape");
    } finally {
      await close();
    }
  });

  test("View and Help, and the keyboard: Alt, F10, arrows, Alt+letter", async () => {
    test.setTimeout(120_000);
    const { app, page, close } = await launchFlow(work.dir);
    try {
      // Toggle full screen by click, then F11.
      await chooseMenu(page, app, "View", "Toggle full screen");
      await expect.poll(async () => (await windowState(app)).fullScreen).toBe(true);
      await page.keyboard.press("F11");
      await expect.poll(async () => (await windowState(app)).fullScreen).toBe(false);

      // Toggle developer tools (a development build) by click, then Ctrl+Shift+I.
      await chooseMenu(page, app, "View", "Toggle developer tools");
      await expect.poll(async () => (await windowState(app)).devTools).toBe(true);
      await page.keyboard.press("Control+Shift+I");
      await expect.poll(async () => (await windowState(app)).devTools).toBe(false);

      // Reload by click, then Ctrl+R: the page loads again and reconnects to the engine.
      for (const how of ["click", "key"] as const) {
        const loaded = page.waitForEvent("load");
        if (how === "click") await chooseMenu(page, app, "View", "Reload");
        else await page.keyboard.press("Control+R");
        await loaded;
        await editorHandle(page);
        await expect.poll(async () => (await editorState(page)).connection).toBe("ready");
        await expect(menuBar(page)).toBeVisible();
      }

      // Help > About Cutweave shows the versions main reports; Close dismisses it.
      const versions = await app.evaluate(({ app: a }) => ({
        version: a.getVersion(),
        electron: process.versions.electron,
      }));
      await chooseMenu(page, app, "Help", "About Cutweave");
      const about = page.getByRole("dialog", { name: "About Cutweave" });
      await expect(about).toBeVisible();
      await expect(about.getByTestId("about-versions")).toContainText(versions.version);
      await expect(about.getByTestId("about-versions")).toContainText(versions.electron);
      await about.getByRole("button", { name: "Close" }).click();
      await expect(about).toBeHidden();

      // Alt alone: the first menu takes the focus and the access letters show; arrows move,
      // Down opens, Escape closes, a second Escape leaves the bar.
      const bar = menuBar(page);
      await page.keyboard.press("Alt");
      await expect(menuTrigger(page, "File")).toBeFocused();
      await expect(bar).toHaveAttribute("data-access-keys", "");
      await page.keyboard.press("ArrowRight");
      await expect(menuTrigger(page, "Edit")).toBeFocused();
      await page.keyboard.press("ArrowDown");
      const edit = page.getByRole("menu", { name: "Edit" });
      await expect(edit).toBeVisible();
      await expect(item(edit, "Cut")).toBeVisible();
      await page.keyboard.press("ArrowRight");
      await expect(page.getByRole("menu", { name: "View" })).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(page.getByRole("menu")).toHaveCount(0);
      await expect(menuTrigger(page, "View")).toBeFocused();
      await page.keyboard.press("Escape");
      await expect(menuTrigger(page, "View")).not.toBeFocused();
      await expect(bar).not.toHaveAttribute("data-access-keys", "");

      // F10 does the same; a second F10 leaves.
      await page.keyboard.press("F10");
      await expect(menuTrigger(page, "File")).toBeFocused();
      await page.keyboard.press("F10");
      await expect(menuTrigger(page, "File")).not.toBeFocused();

      // Alt+H opens Help with its first command focused; Enter runs it.
      await page.keyboard.press("Alt+H");
      const help = page.getByRole("menu", { name: "Help" });
      await expect(help).toBeVisible();
      await expect(item(help, "About Cutweave")).toBeFocused();
      await page.keyboard.press("Enter");
      await expect(about).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(about).toBeHidden();

      // Alt+V, Enter: full screen from the keyboard alone.
      await page.keyboard.press("Alt+V");
      await expect(
        item(page.getByRole("menu", { name: "View" }), "Toggle full screen"),
      ).toBeFocused();
      await page.keyboard.press("Enter");
      await expect.poll(async () => (await windowState(app)).fullScreen).toBe(true);
      await page.keyboard.press("F11");
      await expect.poll(async () => (await windowState(app)).fullScreen).toBe(false);
    } finally {
      await close();
    }
  });

  test("File > Exit asks about unsaved changes: Cancel stays, Save quits with them on disk", async () => {
    test.setTimeout(90_000);
    const { app, page, close } = await launchFlow(work.dir);
    try {
      await page.keyboard.press("Control+N");
      const doc = await nameProject(page, "New project", "Leaving", work.projects);
      await importClip(app, page, work.clip);
      await expect(dirtyDot(page)).toBeVisible();
      expect(onDisk(doc).media).toBe(0);

      // Exit, then Cancel: the app stays with its unsaved change.
      await chooseMenu(page, app, "File", "Exit");
      const prompt = unsavedPrompt(page);
      await expect(prompt.dialog).toBeVisible();
      await prompt.cancel.click();
      await expect(prompt.dialog).toBeHidden();
      await expect(dirtyDot(page)).toBeVisible();
      expect(app.process().exitCode).toBeNull();

      // Exit, then Save: the app quits and the media is in the document on disk.
      const exited = new Promise<number | null>((resolve) =>
        app.process().once("exit", (code) => resolve(code)),
      );
      await chooseMenu(page, app, "File", "Exit");
      await expect(prompt.dialog).toBeVisible();
      await prompt.save.click();
      expect(await exited).toBe(0);
      expect(onDisk(doc).media).toBe(1);
    } finally {
      await close();
    }
  });

  test("the title bar: no native menu, OS controls in a reserved overlay, drag and double-click", async () => {
    test.setTimeout(90_000);
    const { app, page, close } = await launchFlow(work.dir);
    try {
      // No native menu on Windows and Linux; macOS keeps its app menu.
      const native = await app.evaluate(({ Menu }) => Menu.getApplicationMenu() !== null);
      expect(native).toBe(process.platform === "darwin");
      await expect(menuBar(page)).toHaveCount(process.platform === "darwin" ? 0 : 1);

      // The overlay is on, as tall as the top bar, and nothing in the bar sits under it.
      const geometry = await page.evaluate(() => {
        // The Window Controls Overlay API (not in TypeScript's DOM types yet).
        const wco = (
          navigator as Navigator & {
            windowControlsOverlay: { visible: boolean; getTitlebarAreaRect(): DOMRect };
          }
        ).windowControlsOverlay;
        const area = wco.getTitlebarAreaRect();
        const bar = document.querySelector(".cw-topbar") as HTMLElement;
        const controls = [...bar.querySelectorAll("button, [role='menubar'], [role='radiogroup']")];
        const style = (el: Element) =>
          getComputedStyle(el).getPropertyValue("-webkit-app-region") ||
          getComputedStyle(el).getPropertyValue("app-region");
        return {
          visible: wco.visible,
          area: { x: area.x, y: area.y, width: area.width, height: area.height },
          barHeight: bar.getBoundingClientRect().height,
          viewport: window.innerWidth,
          rightmost: Math.max(...controls.map((c) => c.getBoundingClientRect().right)),
          leftmost: Math.min(...controls.map((c) => c.getBoundingClientRect().left)),
          barRegion: style(bar),
          controlRegions: [...new Set(controls.map(style))],
        };
      });
      expect(geometry.visible).toBe(true);
      expect(geometry.area.height).toBe(geometry.barHeight);
      expect(geometry.barHeight).toBe(40);
      expect(geometry.area.width).toBeLessThan(geometry.viewport);
      expect(geometry.rightmost).toBeLessThanOrEqual(geometry.area.x + geometry.area.width);
      expect(geometry.leftmost).toBeGreaterThanOrEqual(geometry.area.x);
      expect(geometry.barRegion).toBe("drag");
      expect(geometry.controlRegions).toEqual(["no-drag"]);

      test.skip(!osInputAvailable, "real OS mouse input and screen pixels: Windows only");
      // The person looks at the app: bring its window in front of everything else on the desktop
      // for the real-mouse steps (a window manager setting; the app itself is not changed). A
      // press over any other app's window aborts instead (`os-input.ts`).
      const owner = await ownerOf(app);
      await app.evaluate(({ BrowserWindow }) => {
        const win = BrowserWindow.getAllWindows()[0];
        win?.setAlwaysOnTop(true, "screen-saver");
        win?.show();
        win?.focus();
      });

      // The OS draws the window controls in the theme's frame color, and follows a switch.
      const controlsAt = await screenPoint(app, {
        x: geometry.area.x + geometry.area.width + 4,
        y: 4,
      });
      const park = await screenPoint(app, { x: geometry.viewport / 2, y: 400 });
      await expect.poll(() => osPixel(owner, controlsAt, park)).toBe(colors.dark["surface-0"]);
      await chooseTheme(page, app, "Light");
      await expect.poll(() => osPixel(owner, controlsAt, park)).toBe(colors.light["surface-0"]);
      await chooseTheme(page, app, "Dark");
      await expect.poll(() => osPixel(owner, controlsAt, park)).toBe(colors.dark["surface-0"]);

      // An empty spot of the title bar: the space before the engine status.
      const grip: Point = await page.evaluate(() => {
        const end = document.querySelector(".cw-topbar__group--end") as HTMLElement;
        const first = end.firstElementChild as HTMLElement;
        const left = end.getBoundingClientRect().left;
        return { x: Math.round((left + first.getBoundingClientRect().left) / 2), y: 20 };
      });
      const hit = await page.evaluate(
        ({ x, y }) => document.elementFromPoint(x, y)?.className ?? "",
        grip,
      );
      expect(hit).toContain("cw-topbar__group");

      // Click it first, as a person does to bring a window forward (Windows does not let a
      // background process take the foreground itself): the window becomes the active one.
      await toFront(app);
      osClick(owner, await screenPoint(app, grip));
      await expect.poll(async () => (await windowState(app)).focused).toBe(true);

      // Drag it: the window moves by the distance dragged.
      const before = await windowState(app);
      expect(before.maximized).toBe(false);
      const scale = await app.evaluate(({ screen, BrowserWindow }) => {
        const win = BrowserWindow.getAllWindows()[0];
        return win === undefined ? 1 : screen.getDisplayMatching(win.getBounds()).scaleFactor;
      });
      const from = await screenPoint(app, grip);
      const by = { x: Math.round(120 * scale), y: Math.round(80 * scale) };
      const nudge = { x: Math.round(8 * scale), y: Math.round(5 * scale) };
      await toFront(app);
      osDrag(owner, from, { x: from.x + by.x, y: from.y + by.y }, nudge);
      // The OS starts moving the window once the pointer leaves its drag threshold (the nudge);
      // from there the window follows the pointer.
      await expect
        .poll(async () => (await windowState(app)).bounds.x !== before.bounds.x)
        .toBe(true);
      const moved = (await windowState(app)).bounds;
      const dx = moved.x - before.bounds.x;
      const dy = moved.y - before.bounds.y;
      expect(dx, `moved by ${dx}, ${dy}`).toBeGreaterThanOrEqual(120 - 8 - 1);
      expect(dx, `moved by ${dx}, ${dy}`).toBeLessThanOrEqual(120);
      expect(dy, `moved by ${dx}, ${dy}`).toBeGreaterThanOrEqual(80 - 5 - 1);
      expect(dy, `moved by ${dx}, ${dy}`).toBeLessThanOrEqual(80);
      expect({ width: moved.width, height: moved.height }).toEqual({
        width: before.bounds.width,
        height: before.bounds.height,
      });

      // Double-click it: maximized; again: restored to where it was.
      await toFront(app);
      osDoubleClick(owner, await screenPoint(app, grip));
      await expect.poll(async () => (await windowState(app)).maximized).toBe(true);
      const maxGrip = await page.evaluate(() => {
        const end = document.querySelector(".cw-topbar__group--end") as HTMLElement;
        const first = end.firstElementChild as HTMLElement;
        const left = end.getBoundingClientRect().left;
        return { x: Math.round((left + first.getBoundingClientRect().left) / 2), y: 20 };
      });
      await toFront(app);
      osDoubleClick(owner, await screenPoint(app, maxGrip));
      await expect.poll(async () => (await windowState(app)).maximized).toBe(false);
      await expect.poll(async () => (await windowState(app)).bounds).toEqual(moved);
      await app.evaluate(({ BrowserWindow }) =>
        BrowserWindow.getAllWindows()[0]?.setAlwaysOnTop(false),
      );
    } finally {
      await close();
    }
  });
});
