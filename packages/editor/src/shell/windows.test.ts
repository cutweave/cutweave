// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// The editor's window registry (WP-2H.14): the main window and each detached panel's window.
import { afterEach, describe, expect, it } from "vitest";
import { sessionStore } from "../session/sessionStore.ts";
import { docStore } from "../state/docStore.ts";
import {
  activeDocument,
  activeWindow,
  addPopoutWindow,
  documentOf,
  forEveryWindow,
  isKeptOnTop,
  isMainWindow,
  MIRRORED_ATTRIBUTES,
  onDidAddWindow,
  onDidRemoveWindow,
  popoutTitle,
  registeredWindows,
  removePopoutWindow,
  resetWindowRegistry,
  setKeptOnTop,
  setPopoutPanels,
  windowEntry,
  windowOf,
} from "./windows.ts";

/** A second window: an iframe's, named the way dockview names a popout. */
function secondWindow(name = "1-7"): Window {
  const frame = document.createElement("iframe");
  document.body.append(frame);
  const win = frame.contentWindow;
  if (win === null) throw new Error("no iframe window");
  win.name = name;
  return win;
}

const flush = () => new Promise((done) => setTimeout(done, 0));

afterEach(() => {
  resetWindowRegistry();
  for (const frame of document.querySelectorAll("iframe")) frame.remove();
  for (const name of MIRRORED_ATTRIBUTES) document.documentElement.removeAttribute(name);
  sessionStore.setState({ known: false, name: "" });
  docStore.setState({ project: null });
});

describe("the window registry", () => {
  it("always holds the main window, then each popout until it closes", () => {
    expect(registeredWindows().map((w) => [w.id, w.role])).toEqual([["main", "main"]]);
    const added: string[] = [];
    const removed: string[] = [];
    onDidAddWindow((w) => added.push(w.id));
    const stop = onDidRemoveWindow((w) => removed.push(w.id));
    const popout = secondWindow();
    const entry = addPopoutWindow(popout, ["panel.inspector"]);
    expect(entry).toMatchObject({ id: "1-7", role: "popout", window: popout });
    expect(windowEntry(popout)).toBe(entry);
    expect(windowEntry(window)?.role).toBe("main");
    // Registering it again only updates it.
    expect(addPopoutWindow(popout, ["panel.inspector"])).toBe(entry);
    expect(added).toEqual(["1-7"]);
    expect(registeredWindows().map((w) => w.id)).toEqual(["main", "1-7"]);
    removePopoutWindow(popout);
    removePopoutWindow(popout);
    expect(removed).toEqual(["1-7"]);
    stop();
    expect(registeredWindows().map((w) => w.id)).toEqual(["main"]);
  });

  it("gives each popout the main window's theme, density, language and direction, live", async () => {
    const html = document.documentElement;
    html.dataset.theme = "dark";
    html.dataset.density = "compact";
    html.lang = "en";
    const popout = secondWindow();
    const target = popout.document.documentElement;
    target.setAttribute("dir", "ltr");
    addPopoutWindow(popout, ["panel.inspector"]);
    expect(target.dataset.theme).toBe("dark");
    expect(target.dataset.density).toBe("compact");
    expect(target.lang).toBe("en");
    // The main window has no `dir`: neither does the popout.
    expect(target.hasAttribute("dir")).toBe(false);
    html.dataset.theme = "light";
    html.dir = "rtl";
    await flush();
    expect(target.dataset.theme).toBe("light");
    expect(target.dir).toBe("rtl");
    // A closed popout no longer follows.
    removePopoutWindow(popout);
    html.dataset.theme = "dark";
    await flush();
    expect(target.dataset.theme).toBe("light");
  });

  it("titles a popout by its panels and the project: Inspector — My video — Cutweave", async () => {
    const popout = secondWindow();
    addPopoutWindow(popout, ["panel.inspector"]);
    expect(popout.document.title).toBe("Inspector — Cutweave");
    sessionStore.setState({ known: true, name: "" });
    await flush();
    expect(popout.document.title).toBe("Inspector — Untitled project — Cutweave");
    sessionStore.setState({ known: true, name: "My video" });
    await flush();
    expect(popout.document.title).toBe("Inspector — My video — Cutweave");
    setPopoutPanels(popout, ["panel.inspector", "panel.history"]);
    expect(popout.document.title).toBe("Inspector, History — My video — Cutweave");
    expect(popoutTitle(["panel.timeline"])).toBe("Timeline — My video — Cutweave");
  });

  it("remembers Keep on top per popout", () => {
    const popout = secondWindow();
    addPopoutWindow(popout);
    expect(isKeptOnTop(popout)).toBe(false);
    setKeptOnTop(popout, true);
    expect(isKeptOnTop(popout)).toBe(true);
    expect(isKeptOnTop(window)).toBe(false);
  });
});

// WP-2H.15: every panel works in its own window.
describe("the per-window helpers", () => {
  it("give a node its own window and document, and the main ones for none", () => {
    const popout = secondWindow();
    const moved = popout.document.createElement("canvas");
    popout.document.body.append(moved);
    expect(windowOf(moved)).toBe(popout);
    expect(documentOf(moved)).toBe(popout.document);
    expect(windowOf(popout.document)).toBe(popout);
    expect(documentOf(popout.document)).toBe(popout.document);
    // A node made in the main window and moved into the other one (what the dock does) follows.
    const adopted = document.createElement("div");
    popout.document.body.append(adopted);
    expect(windowOf(adopted)).toBe(popout);
    expect(windowOf(document.body)).toBe(window);
    expect(windowOf(null)).toBe(window);
    expect(documentOf(undefined)).toBe(document);
    expect(isMainWindow(window)).toBe(true);
    expect(isMainWindow(popout)).toBe(false);
  });

  it("know the window the person works in: the one that last had their keys or pointer", () => {
    const popout = secondWindow();
    expect(activeWindow()).toBe(window);
    addPopoutWindow(popout, ["panel.timeline"]);
    popout.dispatchEvent(new KeyboardEvent("keydown", { key: "k" }));
    expect(activeWindow()).toBe(popout);
    expect(activeDocument()).toBe(popout.document);
    window.dispatchEvent(new PointerEvent("pointerdown"));
    expect(activeWindow()).toBe(window);
    popout.dispatchEvent(new FocusEvent("focus"));
    expect(activeWindow()).toBe(popout);
    // A window that closed is not where the person is.
    removePopoutWindow(popout);
    expect(activeWindow()).toBe(window);
  });

  it("install on every window, as windows open, and undo it as they close", () => {
    const first = secondWindow("1-1");
    addPopoutWindow(first);
    const installed: Window[] = [];
    const removed: Window[] = [];
    const stop = forEveryWindow((win) => {
      installed.push(win);
      return () => removed.push(win);
    });
    expect(installed).toEqual([window, first]);
    const second = secondWindow("1-2");
    addPopoutWindow(second);
    expect(installed).toEqual([window, first, second]);
    removePopoutWindow(first);
    expect(removed).toEqual([first]);
    stop();
    expect(removed).toEqual([first, window, second]);
    // Stopped: a window opening later gets nothing.
    addPopoutWindow(secondWindow("1-3"));
    expect(installed).toHaveLength(3);
  });
});
