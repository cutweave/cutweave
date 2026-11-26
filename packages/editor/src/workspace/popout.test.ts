// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-2H.14 (ADR-0031): a panel in a window of its own and back. Drives the real shell and the real
// dock in jsdom; `window.open` hands out fake windows (an iframe's document behind a window-shaped
// object), which is what dockview moves a popout group into.

import { defaultPreferences } from "@cutweave/preferences";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { LAYOUT_SAVE_DELAY_MS } from "../dock/Dock.tsx";
import { prefsStore } from "../prefs/prefsStore.ts";
import { EditorShell } from "../shell/EditorShell.tsx";
import {
  onDidAddWindow,
  onDidRemoveWindow,
  registeredWindows,
  resetWindowRegistry,
} from "../shell/windows.ts";
import { toastStore } from "../state/toastStore.ts";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import { listWorkspaceCommands, runWorkspaceCommand } from "./commands.ts";
import {
  currentDock,
  detachedPanels,
  dockPanel,
  isPanelDetached,
  POPOUT_URL,
  popoutPanel,
  resetDockHost,
  saveLayoutNow,
} from "./dockHost.ts";
import { dragOutPoint } from "./dragOut.ts";
import { readLayoutMeta } from "./layoutMeta.ts";

/** A window `window.open` handed out: a real (iframe) document behind a window-shaped object. */
interface FakeWindow {
  readonly name: string;
  readonly document: Document;
  readonly features: string;
  closed: boolean;
  /** The person closes it with the OS close button: the page unloads (`beforeunload`). */
  closeByUser(): void;
}

let opened: FakeWindow[] = [];
/** What `window.open` answers: a window, or `null` as when the OS or a policy refuses it. */
let refuse = false;

function fakeWindow(name: string, features: string): FakeWindow {
  const frame = document.createElement("iframe");
  document.body.append(frame);
  const doc = frame.contentDocument;
  if (doc === null) throw new Error("no iframe document");
  const events = new EventTarget();
  const fake = {
    name,
    features,
    document: doc,
    closed: false,
    screenX: 1200,
    screenY: 80,
    innerWidth: 320,
    innerHeight: 600,
    outerWidth: 320,
    outerHeight: 640,
    addEventListener: events.addEventListener.bind(events),
    removeEventListener: events.removeEventListener.bind(events),
    dispatchEvent: events.dispatchEvent.bind(events),
    focus: () => {},
    close() {
      // Script closed it: the dock notices on its next check of `closed`.
      fake.closed = true;
    },
    closeByUser() {
      if (fake.closed) return;
      events.dispatchEvent(new Event("beforeunload"));
      fake.closed = true;
    },
  };
  // The page loads after `window.open` returns, as a real one does.
  setTimeout(() => events.dispatchEvent(new Event("load")), 0);
  return fake;
}

beforeAll(() => {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;
  globalThis.requestAnimationFrame ??= (cb: FrameRequestCallback) =>
    setTimeout(() => cb(performance.now()), 16) as unknown as number;
  globalThis.cancelAnimationFrame ??= (id: number) => clearTimeout(id);
  globalThis.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
    onchange: null,
  })) as unknown as typeof matchMedia;
  Element.prototype.scrollIntoView ??= () => {};
});

beforeEach(() => {
  opened = [];
  refuse = false;
  vi.spyOn(window, "open").mockImplementation((_url, target, features) => {
    if (refuse) return null;
    const fake = fakeWindow(String(target ?? ""), String(features ?? ""));
    opened.push(fake);
    return fake as unknown as Window;
  });
  viewStore.setState({ ...initialViewState, layouts: {} });
  prefsStore.setState({ values: defaultPreferences() });
  toastStore.getState().clear();
});

afterEach(() => {
  cleanup();
  resetDockHost();
  resetWindowRegistry();
  vi.restoreAllMocks();
  delete document.documentElement.dataset.theme;
  delete (globalThis as { cutweave?: unknown }).cutweave;
  for (const frame of document.querySelectorAll("iframe")) frame.remove();
});

const mainRegion = (name: string) => screen.queryByRole("region", { name });
const popoutRegion = (win: FakeWindow, name: string) =>
  within(win.document.body).queryByRole("region", { name });

async function allPanelsShown() {
  for (const name of ["Assets", "Preview", "Inspector", "Timeline"]) {
    expect(await screen.findByRole("region", { name })).toBeTruthy();
  }
}

/** Right-click the panel's tab and choose `item` in its menu. */
async function tabMenu(user: ReturnType<typeof userEvent.setup>, panel: string, item: string) {
  await user.pointer({ keys: "[MouseRight]", target: screen.getByRole("tab", { name: panel }) });
  await user.click(await screen.findByText(item));
}

/** The Inspector, moved into its own window from its tab's menu. */
async function detachInspector(user: ReturnType<typeof userEvent.setup>): Promise<FakeWindow> {
  await tabMenu(user, "Inspector", "Move to new window");
  await waitFor(() => expect(isPanelDetached("inspector")).toBe(true));
  const win = opened.at(-1);
  if (win === undefined) throw new Error("no window opened");
  return win;
}

const savedPopouts = () => viewStore.getState().layouts["short-form"]?.popoutGroups ?? [];

describe("moving a panel into its own window (WP-2H.14)", () => {
  it("opens the popout page in a new window named by the dock, and moves the panel there", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await allPanelsShown();
    const added: string[] = [];
    onDidAddWindow((w) => added.push(w.role));

    const win = await detachInspector(user);
    expect(window.open).toHaveBeenCalledWith(POPOUT_URL, expect.anything(), expect.any(String));
    // dockview's window name, `<dock>-<group>`: the one main allows (popout.ts).
    expect(win.name).toMatch(/^\d{1,9}-\d{1,9}$/);
    expect(
      await within(win.document.body).findByRole("region", { name: "Inspector" }),
    ).toBeTruthy();
    expect(mainRegion("Inspector")).toBeNull();
    expect(mainRegion("Preview")).toBeTruthy();

    // The registry holds it, titled by its panel, in the main window's look.
    expect(added).toEqual(["popout"]);
    expect(registeredWindows().map((w) => w.role)).toEqual(["main", "popout"]);
    expect(win.document.title).toBe("Inspector — Cutweave");
    expect(win.document.documentElement.dataset.theme).toBe("dark");
    expect(win.document.documentElement.dataset.density).toBe("compact");
    expect(win.document.documentElement.lang).toBe("en");

    // The saved layout has the window, and the spot the panel left.
    await waitFor(() => expect(savedPopouts()).toHaveLength(1));
    expect(savedPopouts()[0]?.url).toBe(POPOUT_URL);
    expect(readLayoutMeta(viewStore.getState().layouts["short-form"]).lastDock.inspector).toEqual(
      expect.objectContaining({ ref: "preview", direction: "right" }),
    );
  });

  it("follows the main window's theme", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await allPanelsShown();
    const win = await detachInspector(user);
    prefsStore.setState({ values: { ...defaultPreferences(), "appearance.theme": "light" } });
    await waitFor(() => expect(win.document.documentElement.dataset.theme).toBe("light"));
    document.documentElement.setAttribute("dir", "rtl");
    await waitFor(() => expect(win.document.documentElement.getAttribute("dir")).toBe("rtl"));
    document.documentElement.removeAttribute("dir");
  });

  it("docks the panel back where it was when the person closes its window", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await allPanelsShown();
    const removed: string[] = [];
    onDidRemoveWindow((w) => removed.push(w.role));
    const win = await detachInspector(user);
    await waitFor(() => expect(savedPopouts()).toHaveLength(1));

    win.closeByUser();
    expect(await screen.findByRole("region", { name: "Inspector" })).toBeTruthy();
    expect(isPanelDetached("inspector")).toBe(false);
    // Right of the Preview again: after it in the DOM order of the groups.
    const order = screen.getAllByRole("region").map((r) => r.getAttribute("aria-label"));
    expect(order.indexOf("Inspector")).toBeGreaterThan(order.indexOf("Preview"));
    expect(removed).toEqual(["popout"]);
    expect(registeredWindows().map((w) => w.role)).toEqual(["main"]);
    await waitFor(() => expect(savedPopouts()).toHaveLength(0), {
      timeout: LAYOUT_SAVE_DELAY_MS * 10,
    });
    // Its tab, a new one in its old group, still has the tab's menu: it can go out again.
    await detachInspector(user);
    expect(opened).toHaveLength(2);
  });

  it("moves it back from the menu's command, and from the palette for the active panel", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await allPanelsShown();
    const win = await detachInspector(user);

    dockPanel("inspector");
    expect(win.closed).toBe(true);
    await waitFor(() => expect(mainRegion("Inspector")).toBeTruthy());
    expect(detachedPanels()).toEqual([]);

    // The palette and keys run the commands with no panel: the active one.
    currentDock()?.getPanel("assets")?.api.setActive();
    expect(runWorkspaceCommand("workspace.popout_panel")).toBe(true);
    await waitFor(() => expect(isPanelDetached("assets")).toBe(true));
    expect(runWorkspaceCommand("workspace.dock_panel", { panel: "assets" })).toBe(true);
    await waitFor(() => expect(mainRegion("Assets")).toBeTruthy());
    const ids = listWorkspaceCommands().map((c) => c.id);
    expect(ids).toContain("workspace.popout_panel");
    expect(ids).toContain("workspace.dock_panel");
  });

  it("keeps a window's other panels in it when one goes back", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await allPanelsShown();
    const win = await detachInspector(user);
    // A second panel joins the Inspector's window (as a dropped tab would).
    const dock = currentDock();
    const assets = dock?.getPanel("assets");
    const inspector = dock?.getPanel("inspector");
    if (assets === undefined || inspector === undefined) throw new Error("no panels");
    assets.api.moveTo({ group: inspector.group });
    await waitFor(() => expect(popoutRegion(win, "Assets")).toBeTruthy());
    await waitFor(() => expect(win.document.title).toBe("Inspector, Assets — Cutweave"));

    dockPanel("assets");
    await waitFor(() => expect(mainRegion("Assets")).toBeTruthy());
    expect(win.closed).toBe(false);
    expect(isPanelDetached("inspector")).toBe(true);
    await waitFor(() => expect(win.document.title).toBe("Inspector — Cutweave"));
  });

  it("reopens the window after a relaunch, from the saved layout", async () => {
    const user = userEvent.setup();
    const first = render(h(EditorShell));
    await allPanelsShown();
    await detachInspector(user);
    await waitFor(() => expect(savedPopouts()).toHaveLength(1));
    first.unmount();
    resetDockHost();
    resetWindowRegistry();

    const persisted = JSON.parse(JSON.stringify(viewStore.getState().layouts));
    viewStore.setState({ ...initialViewState, layouts: persisted });
    opened = [];
    render(h(EditorShell));
    await waitFor(() => expect(isPanelDetached("inspector")).toBe(true));
    const win = opened[0];
    if (win === undefined) throw new Error("no window reopened");
    expect(win.name).toMatch(/^\d{1,9}-\d{1,9}$/);
    expect(win.features).toContain("left=1200");
    expect(popoutRegion(win, "Inspector")).toBeTruthy();
    expect(mainRegion("Inspector")).toBeNull();
    expect(registeredWindows().map((w) => w.role)).toEqual(["main", "popout"]);
    // The restored layout is saved again only once the window is open: it still has it.
    await waitFor(() => expect(savedPopouts()).toHaveLength(1));
  });

  it("does not save the layout the dock leaves while the main window unloads", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await allPanelsShown();
    await detachInspector(user);
    await waitFor(() => expect(savedPopouts()).toHaveLength(1));
    // The dock closes its windows on the main window's `beforeunload`, docking their panels.
    window.dispatchEvent(new Event("beforeunload"));
    await waitFor(() => expect(mainRegion("Inspector")).toBeTruthy());
    saveLayoutNow();
    await new Promise((done) => setTimeout(done, LAYOUT_SAVE_DELAY_MS * 2));
    expect(savedPopouts()).toHaveLength(1);
  });

  it("keeps the panel docked, and says so, when the window cannot open", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await allPanelsShown();
    refuse = true;
    await tabMenu(user, "Inspector", "Move to new window");
    await waitFor(() =>
      expect(toastStore.getState().notices.map((n) => n.title)).toContain(
        "The panel could not open in a new window",
      ),
    );
    expect(isPanelDetached("inspector")).toBe(false);
    expect(mainRegion("Inspector")).toBeTruthy();
    expect(await popoutPanel("inspector")).toBe(false);
    expect(registeredWindows().map((w) => w.role)).toEqual(["main"]);
  });

  it("docks every window back on Reset layout", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await allPanelsShown();
    const win = await detachInspector(user);
    await user.click(screen.getByRole("menuitem", { name: "View" }));
    await user.click(await screen.findByRole("menuitem", { name: "Reset layout" }));
    await allPanelsShown();
    expect(win.closed).toBe(true);
    expect(detachedPanels()).toEqual([]);
    await waitFor(() => expect(registeredWindows().map((w) => w.role)).toEqual(["main"]));
  });

  it("closes a workspace's windows when switching away, and reopens them on the way back", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await allPanelsShown();
    const win = await detachInspector(user);
    await user.click(screen.getByRole("radio", { name: "Long-form" }));
    await waitFor(() => expect(win.closed).toBe(true));
    await waitFor(() => expect(mainRegion("Inspector")).toBeTruthy());
    expect(detachedPanels()).toEqual([]);

    await user.click(screen.getByRole("radio", { name: "Short-form" }));
    await waitFor(() => expect(isPanelDetached("inspector")).toBe(true));
    expect(opened).toHaveLength(2);
  });

  it("offers Move to new window in a docked tab's menu, and Move back in a detached one's", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await allPanelsShown();
    await user.pointer({
      keys: "[MouseRight]",
      target: screen.getByRole("tab", { name: "Preview" }),
    });
    expect(await screen.findByText("Move to new window")).toBeTruthy();
    expect(screen.queryByText("Move back to main window")).toBeNull();
    await user.keyboard("{Escape}");
    const win = await detachInspector(user);
    // A docked tab still has no way back: it is not out (`panel.detached`, WP-2H.15).
    await user.pointer({
      keys: "[MouseRight]",
      target: screen.getByRole("tab", { name: "Preview" }),
    });
    expect(await screen.findByText("Move to new window")).toBeTruthy();
    expect(screen.queryByText("Move back to main window")).toBeNull();
    await user.keyboard("{Escape}");
    // The detached tab's menu opens in its own window: Move back, and no Move to new window.
    const popout = within(win.document.body);
    fireEvent.contextMenu(popout.getByRole("tab", { name: "Inspector" }), {
      clientX: 20,
      clientY: 10,
    });
    const back = (await popout.findByText("Move back to main window")).closest("[role='menuitem']");
    expect(back?.getAttribute("aria-disabled")).toBeNull();
    expect(popout.queryByText("Move to new window")).toBeNull();
    expect(screen.queryByRole("menu")).toBeNull();
    fireEvent.click(back as Element);
    await waitFor(() => expect(isPanelDetached("inspector")).toBe(false));
  });

  it("toggles Keep on top in the detached window's header (desktop app only)", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await allPanelsShown();
    const setPopoutOnTop = vi.fn(async () => true);
    (globalThis as { cutweave?: unknown }).cutweave = { window: { setPopoutOnTop } };
    const win = await detachInspector(user);
    const pin = await within(win.document.body).findByRole("button", { name: "Keep on top" });
    expect(pin.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(pin);
    await waitFor(() => expect(pin.getAttribute("aria-pressed")).toBe("true"));
    expect(setPopoutOnTop).toHaveBeenCalledWith(win.name, true);
    // The main window's groups have no such button.
    expect(screen.queryByRole("button", { name: "Keep on top" })).toBeNull();
  });

  it("opens a window where a tab dragged out of the main window was let go", async () => {
    render(h(EditorShell));
    await allPanelsShown();
    const tab = screen.getByRole("tab", { name: "Inspector" });
    const drag = (type: string, init: MouseEventInit, dropEffect: string) => {
      const event = new MouseEvent(type, { bubbles: true, cancelable: true, ...init });
      const dataTransfer = {
        dropEffect,
        effectAllowed: "all",
        items: [],
        setData: () => {},
        setDragImage: () => {},
      };
      Object.defineProperty(event, "dataTransfer", { value: dataTransfer });
      tab.dispatchEvent(event);
    };
    // Let go over the main window (jsdom's is at 0, 0, 1024 x 768): nothing happens.
    drag("dragstart", {}, "move");
    drag("dragend", { screenX: 500, screenY: 300 }, "none");
    expect(opened).toHaveLength(0);
    // Let go outside it with no drop: a window opens there.
    drag("dragstart", {}, "move");
    drag("dragend", { screenX: 1500, screenY: 200 }, "none");
    await waitFor(() => expect(isPanelDetached("inspector")).toBe(true));
    expect(opened[0]?.features).toContain("left=1500");
    expect(opened[0]?.features).toContain("top=200");
  });
});

describe("reading a tab drag that ends outside the windows", () => {
  const main = { left: 0, top: 0, width: 1000, height: 800 };
  const popout = { left: 1200, top: 100, width: 300, height: 400 };
  const end = (x: number, y: number, dropEffect = "none") => ({
    screenX: x,
    screenY: y,
    dataTransfer: { dropEffect },
  });

  it("opens at the pointer only when nothing took the drop and no window is under it", () => {
    expect(dragOutPoint(end(1100, 50), [main, popout])).toEqual({ x: 1100, y: 50 });
    expect(dragOutPoint(end(500, 400), [main, popout])).toBeNull();
    expect(dragOutPoint(end(1300, 200), [main, popout])).toBeNull();
    expect(dragOutPoint(end(1100, 50, "move"), [main, popout])).toBeNull();
    // A platform that reports no position (some Linux setups): the menu item still works.
    expect(dragOutPoint(end(0, 0), [popout])).toBeNull();
    expect(dragOutPoint({ ...end(1100, 50), dataTransfer: null }, [main])).toEqual({
      x: 1100,
      y: 50,
    });
  });
});
