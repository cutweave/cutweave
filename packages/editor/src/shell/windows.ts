// SPDX-License-Identifier: GPL-3.0-or-later
// The editor's windows (WP-2H.14, ADR-0031): the main window and each detached panel's window.
// A detached window is a dockview popout: a blank `popout.html` in the editor's own renderer that
// the dock moves a panel group into, while React, the stores and the engine connection stay in the
// main window. This registry keeps each popout looking like the main window (theme, density,
// language and direction on its `<html>`) and titles it by its panels ("Inspector — My video —
// Cutweave"). WP-2H.15 installs the keys, menus and tooltips on every window through
// `registeredWindows()` and `onDidAddWindow`.
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

function mainEntry(): RegisteredWindow | null {
  if (typeof window === "undefined") return null;
  if (main === null || main.window !== window) main = { id: "main", role: "main", window };
  return main;
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
  for (const listener of [...addListeners]) listener(entry);
  return entry;
}

/** A detached window closed (or its panels went back): forget it. */
export function removePopoutWindow(win: Window): void {
  const popout = popouts.get(win);
  if (popout === undefined) return;
  popouts.delete(win);
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

function documentOf(win: Window): Document | null {
  try {
    return win.closed ? null : win.document;
  } catch {
    // A window that went away (or was never ours to script) has nothing to update.
    return null;
  }
}

function mirrorInto(win: Window): void {
  const to = documentOf(win)?.documentElement;
  const from = typeof document === "undefined" ? undefined : document.documentElement;
  if (to === undefined || from === undefined) return;
  for (const name of MIRRORED_ATTRIBUTES) {
    const value = from.getAttribute(name);
    if (value === null) to.removeAttribute(name);
    else if (to.getAttribute(name) !== value) to.setAttribute(name, value);
  }
}

function retitle(win: Window): void {
  const doc = documentOf(win);
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
  main = null;
}
