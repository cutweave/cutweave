// SPDX-License-Identifier: GPL-3.0-or-later
// The editor's windows (WP-2H.14, ADR-0031): the main window and each detached panel's window.
// A detached window is a dockview popout: a blank `popout.html` in the editor's own renderer that
// the dock moves a panel group into, while React, the stores and the engine connection stay in the
// main window. This registry keeps each popout looking like the main window (theme, density,
// language and direction on its `<html>`) and titles it by its panels ("Inspector — My video —
// Cutweave").
//
// Every panel works in every window (WP-2H.15): code that listens for keys or right-clicks
// installs itself on each window with `forEveryWindow`, code that measures or animates a node
// takes the node's own window (`windowOf`, `documentOf`), and "what has the focus" is read in the
// window the person is working in (`activeWindow`, `activeDocument`). A lint rule keeps the bare
// globals (`window`, `document`, `requestAnimationFrame`, ...) out of panel code (biome.json).
import { i18n, type TKey } from "../i18n/index.ts";
import { sessionStore } from "../session/sessionStore.ts";
import { docStore } from "../state/docStore.ts";

export type WindowRole = "main" | "popout";

export interface RegisteredWindow {
  /** `main`, or a popout's window name (dockview's `<dock>-<group>`). */
  readonly id: string;
  readonly role: WindowRole;
  readonly window: Window;
}

/** The `<html>` attributes every window takes from the main one (interface scale joins later). */
export const MIRRORED_ATTRIBUTES = ["data-theme", "data-density", "lang", "dir"] as const;

type Listener = (window: RegisteredWindow) => void;

interface Popout {
  readonly entry: RegisteredWindow;
  /** The panels it holds, by their title keys, in tab order. */
  titles: readonly TKey[];
  /** Keep on top is on (main keeps it; this is what the toggle shows). */
  onTop: boolean;
}

const popouts = new Map<Window, Popout>();
const addListeners = new Set<Listener>();
const removeListeners = new Set<Listener>();
let main: RegisteredWindow | null = null;
/** Stops following the main window; set while at least one popout is open. */
let stopFollowing: (() => void) | null = null;
/** The registered window that last had the person's input or focus. */
let lastActive: Window | null = null;
/** Removes the input tracking from each window that has it. */
const tracking = new Map<Window, () => void>();

/** The main window (the editor's own global), or `null` outside a browser. */
function mainWindow(): Window | null {
  return typeof globalThis.window === "undefined" ? null : globalThis.window;
}

function mainEntry(): RegisteredWindow | null {
  const win = mainWindow();
  if (win === null) return null;
  if (main === null || main.window !== win) {
    main = { id: "main", role: "main", window: win };
    track(win);
  }
  return main;
}

/** Remembers the window that gets the person's input or focus (`activeWindow`). */
function track(win: Window): void {
  if (tracking.has(win)) return;
  const mark = (): void => {
    lastActive = win;
  };
  // Capture on the window: before any other key or pointer listener of that window runs.
  const types = ["focus", "focusin", "pointerdown", "keydown"] as const;
  for (const type of types) win.addEventListener(type, mark, true);
  tracking.set(win, () => {
    for (const type of types) win.removeEventListener(type, mark, true);
  });
}

function untrack(win: Window): void {
  try {
    tracking.get(win)?.();
  } catch {
    // A closed window keeps its listeners; it is gone anyway.
  }
  tracking.delete(win);
  if (lastActive === win) lastActive = null;
}

// ---- per-window helpers (WP-2H.15) -----------------------------------------------------------

/** The window a node is shown in: its document's window, else the main window. */
export function windowOf(node: Node | null | undefined): Window & typeof globalThis {
  const doc =
    node === null || node === undefined
      ? null
      : node.nodeType === 9
        ? (node as Document)
        : node.ownerDocument;
  return (doc?.defaultView ?? mainWindow() ?? globalThis) as Window & typeof globalThis;
}

/** The window an event happened in (its `view`, else its target's window). Realm-safe. */
export function windowOfEvent(event: Event): Window & typeof globalThis {
  const view = (event as Partial<UIEvent>).view;
  if (view !== null && view !== undefined) return view as Window & typeof globalThis;
  const target = event.target as { nodeType?: unknown; window?: unknown } | null;
  if (target !== null && typeof target === "object") {
    if (typeof target.nodeType === "number") return windowOf(target as unknown as Node);
    if (target.window === target) return target as unknown as Window & typeof globalThis;
  }
  return windowOf(null);
}

/** The document a node is in (a detached panel's window has its own), else the main one. */
export function documentOf(node: Node | null | undefined): Document {
  if (node !== null && node !== undefined) {
    if (node.nodeType === 9) return node as Document;
    if (node.ownerDocument !== null) return node.ownerDocument;
  }
  return windowOf(null).document;
}

/**
 * The window the person is working in: the editor window that last had their keys, pointer or
 * focus, else the main window. Menus, the palette and "what has the focus" follow it.
 */
export function activeWindow(): Window & typeof globalThis {
  const first = mainEntry();
  const win = lastActive;
  if (win !== null && (win === first?.window || popouts.has(win)) && !isClosed(win)) {
    return win as Window & typeof globalThis;
  }
  return windowOf(null);
}

/** The document of the window the person is working in (its `activeElement` is their focus). */
export function activeDocument(): Document {
  return activeWindow().document;
}

/**
 * Observes `el`'s size with its own window's `ResizeObserver` (one from another window fires
 * unreliably for it, dockview notes). Returns the function that stops; a no-op where the window
 * has none (tests).
 */
export function observeSize(
  el: Element,
  onResize: ResizeObserverCallback,
  options?: ResizeObserverOptions,
): () => void {
  const Observer = windowOf(el).ResizeObserver ?? globalThis.ResizeObserver;
  if (Observer === undefined) return () => {};
  const observer = new Observer(onResize);
  observer.observe(el, options);
  return () => observer.disconnect();
}

/** Where a loop waits for the next frame: `requestAnimationFrame` and its cancel. */
export interface WindowFrames {
  request(callback: () => void): unknown;
  cancel(handle: unknown): void;
}

/**
 * Animation frames of the window `node()` is in, read at each request: a loop that follows a
 * panel into its own window keeps running there, even while the main window is hidden (a hidden
 * window's frames stop). Without frames (Node tests), a 16 ms timer.
 */
export function framesOf(node: () => Node | null | undefined): WindowFrames {
  interface Pending {
    readonly win: Window;
    readonly id: number;
  }
  return {
    request(callback) {
      const win = windowOf(node());
      if (typeof win.requestAnimationFrame !== "function") return setTimeout(callback, 16);
      return { win, id: win.requestAnimationFrame(() => callback()) } satisfies Pending;
    },
    cancel(handle) {
      if (typeof handle === "object" && handle !== null && "win" in handle) {
        const { win, id } = handle as Pending;
        try {
          win.cancelAnimationFrame(id);
        } catch {
          // The window closed: its frames are gone with it.
        }
        return;
      }
      clearTimeout(handle as ReturnType<typeof setTimeout>);
    },
  };
}

/** Whether `win` is the main window (a node there needs no portal container of its own). */
export function isMainWindow(win: Window | null | undefined): boolean {
  return win === null || win === undefined || win === mainWindow();
}

/**
 * Runs `install` on every editor window: each open one now, and each that opens later. What it
 * returns runs when that window closes, and for every window when the returned function is
 * called. Key and right-click listeners use it, so a detached panel's window has them too.
 */
export function forEveryWindow(install: (win: Window) => (() => void) | undefined): () => void {
  const undo = new Map<Window, () => void>();
  const add = (entry: RegisteredWindow): void => {
    if (undo.has(entry.window)) return;
    const off = install(entry.window);
    undo.set(entry.window, off ?? (() => {}));
  };
  const remove = (win: Window): void => {
    const off = undo.get(win);
    undo.delete(win);
    try {
      off?.();
    } catch {
      // A window that went away cannot be cleaned up, and need not be.
    }
  };
  for (const entry of registeredWindows()) add(entry);
  const offAdd = onDidAddWindow(add);
  const offRemove = onDidRemoveWindow((entry) => remove(entry.window));
  return () => {
    offAdd();
    offRemove();
    for (const win of [...undo.keys()]) remove(win);
  };
}

function isClosed(win: Window): boolean {
  try {
    return win.closed;
  } catch {
    return true;
  }
}

/** The main window first, then each detached window in the order it opened. */
export function registeredWindows(): readonly RegisteredWindow[] {
  const first = mainEntry();
  return [...(first === null ? [] : [first]), ...[...popouts.values()].map((p) => p.entry)];
}

/** Runs `listener` for each window that opens from now on; returns the function that stops it. */
export function onDidAddWindow(listener: Listener): () => void {
  addListeners.add(listener);
  return () => addListeners.delete(listener);
}

/** Runs `listener` for each window that closes from now on; returns the function that stops it. */
export function onDidRemoveWindow(listener: Listener): () => void {
  removeListeners.add(listener);
  return () => removeListeners.delete(listener);
}

/** The registered window `win`, if it is one. */
export function windowEntry(win: Window): RegisteredWindow | undefined {
  return win === mainEntry()?.window ? (main ?? undefined) : popouts.get(win)?.entry;
}

/**
 * Registers a detached panel's window once the dock has moved its panels in: it takes the main
 * window's look and a title naming `titles`. Registering it again only updates the panels.
 */
export function addPopoutWindow(win: Window, titles: readonly TKey[] = []): RegisteredWindow {
  const existing = popouts.get(win);
  if (existing !== undefined) {
    setPopoutPanels(win, titles);
    return existing.entry;
  }
  const entry: RegisteredWindow = {
    id: win.name === "" ? `popout-${popouts.size + 1}` : win.name,
    role: "popout",
    window: win,
  };
  popouts.set(win, { entry, titles, onTop: false });
  mirrorInto(win);
  retitle(win);
  follow();
  mainEntry();
  track(win);
  for (const listener of [...addListeners]) listener(entry);
  return entry;
}

/** A detached window closed (or its panels went back): forget it. */
export function removePopoutWindow(win: Window): void {
  const popout = popouts.get(win);
  if (popout === undefined) return;
  popouts.delete(win);
  untrack(win);
  if (popouts.size === 0) unfollow();
  for (const listener of [...removeListeners]) listener(popout.entry);
}

/** The panels a detached window holds changed (one moved in or out): retitle it. */
export function setPopoutPanels(win: Window, titles: readonly TKey[]): void {
  const popout = popouts.get(win);
  if (popout === undefined) return;
  if (popout.titles.length === titles.length && popout.titles.every((t, i) => t === titles[i])) {
    return;
  }
  popout.titles = titles;
  retitle(win);
}

/** Whether Keep on top is on for this detached window. */
export function isKeptOnTop(win: Window): boolean {
  return popouts.get(win)?.onTop === true;
}

/** Records Keep on top for this detached window (after main applied it). */
export function setKeptOnTop(win: Window, on: boolean): void {
  const popout = popouts.get(win);
  if (popout !== undefined) popout.onTop = on;
}

/** The title of a detached window holding `titles`: "Inspector — My video — Cutweave". */
export function popoutTitle(titles: readonly TKey[]): string {
  const t = i18n.t.bind(i18n);
  const panels = titles.map((key) => t(key)).join(", ");
  const session = sessionStore.getState();
  if (!session.known) return t("windows.popoutTitleNoProject", { panels });
  const engineName = session.name === "" ? t("projectUi.untitled") : session.name;
  const name = docStore.getState().project?.meta.name ?? engineName;
  return t("windows.popoutTitle", { panels, name });
}

function scriptableDocument(win: Window): Document | null {
  try {
    return win.closed ? null : win.document;
  } catch {
    // A window that went away (or was never ours to script) has nothing to update.
    return null;
  }
}

function mirrorInto(win: Window): void {
  const to = scriptableDocument(win)?.documentElement;
  const from = typeof document === "undefined" ? undefined : document.documentElement;
  if (to === undefined || from === undefined) return;
  for (const name of MIRRORED_ATTRIBUTES) {
    const value = from.getAttribute(name);
    if (value === null) to.removeAttribute(name);
    else if (to.getAttribute(name) !== value) to.setAttribute(name, value);
  }
}

function retitle(win: Window): void {
  const doc = scriptableDocument(win);
  const popout = popouts.get(win);
  if (doc === null || popout === undefined) return;
  const title = popoutTitle(popout.titles);
  if (doc.title !== title) doc.title = title;
}

/** Keeps every popout in step with the main window's look, the project's name and the language. */
function follow(): void {
  if (stopFollowing !== null || typeof document === "undefined") return;
  const mirrorAll = () => {
    for (const win of popouts.keys()) mirrorInto(win);
  };
  const retitleAll = () => {
    for (const win of popouts.keys()) retitle(win);
  };
  const observer = new MutationObserver(mirrorAll);
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: [...MIRRORED_ATTRIBUTES],
  });
  const offDoc = docStore.subscribe((s, prev) => {
    if (s.project?.meta.name !== prev.project?.meta.name) retitleAll();
  });
  const offSession = sessionStore.subscribe((s, prev) => {
    if (s.known !== prev.known || s.name !== prev.name) retitleAll();
  });
  i18n.on("languageChanged", retitleAll);
  stopFollowing = () => {
    observer.disconnect();
    offDoc();
    offSession();
    i18n.off("languageChanged", retitleAll);
  };
}

function unfollow(): void {
  stopFollowing?.();
  stopFollowing = null;
}

/** Test seam: forget every popout and listener. */
export function resetWindowRegistry(): void {
  popouts.clear();
  addListeners.clear();
  removeListeners.clear();
  unfollow();
  for (const win of [...tracking.keys()]) untrack(win);
  lastActive = null;
  main = null;
}
