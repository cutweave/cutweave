// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-2H.15: a docked panel knows the window it is in, and follows it as the dock moves it (its
// menus and tooltips open there, its size observers re-observe there).
import { useOwnerWindow, usePortalContainer } from "@cutweave/ui";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { IDockviewPanelProps } from "dockview-react";
import { createElement as h } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import "../i18n/index.ts";
import { inPanelWindow } from "./panelWindow.tsx";

afterEach(() => {
  cleanup();
  for (const frame of document.querySelectorAll("iframe")) frame.remove();
});

/** A panel API whose window the test moves, as the dock does when it detaches the panel. */
function fakeApi() {
  let win: Window = window;
  const listeners = new Set<() => void>();
  const subscribe = (listener: () => void) => {
    listeners.add(listener);
    return { dispose: () => listeners.delete(listener) };
  };
  return {
    api: { getWindow: () => win, onDidLocationChange: subscribe, onDidGroupChange: subscribe },
    moveTo(next: Window) {
      win = next;
      for (const listener of [...listeners]) listener();
    },
  };
}

describe("a docked panel's window", () => {
  it("sets its layers' container to its window and follows it when it moves", async () => {
    const seen: { win: Window | null; container: HTMLElement | null }[] = [];
    function Probe() {
      seen.push({ win: useOwnerWindow(), container: usePortalContainer() });
      return null;
    }
    const Panel = inPanelWindow(Probe);
    // One wrapper per component: the dock never remounts a panel for a new wrapper.
    expect(inPanelWindow(Probe)).toBe(Panel);
    const { api, moveTo } = fakeApi();
    render(h(Panel, { api } as unknown as IDockviewPanelProps));
    await act(async () => {});
    expect(seen.at(-1)).toEqual({ win: window, container: null });

    const frame = document.createElement("iframe");
    document.body.append(frame);
    const popout = frame.contentWindow as Window;
    await act(async () => moveTo(popout));
    expect(seen.at(-1)).toEqual({ win: popout, container: popout.document.body });

    await act(async () => moveTo(window));
    expect(seen.at(-1)).toEqual({ win: window, container: null });
  });
});

describe("a panel that crashes", () => {
  it("shows a compact message with Reload panel, and the panel comes back on reload", async () => {
    const quiet = vi.spyOn(console, "error").mockImplementation(() => {});
    let broken = true;
    function Shaky() {
      if (broken) throw new Error("boom");
      return h("p", null, "all fine");
    }
    const Panel = inPanelWindow(Shaky);
    const { api } = fakeApi();
    render(
      h(Panel, { api, params: { titleKey: "panel.inspector" } } as unknown as IDockviewPanelProps),
    );
    await act(async () => {});
    expect(screen.getByRole("alert").textContent).toContain("stopped working");
    // Reload while still broken: it shows the message again rather than blanking.
    fireEvent.click(screen.getByRole("button", { name: "Reload panel" }));
    expect(screen.getByRole("alert")).toBeTruthy();
    broken = false;
    fireEvent.click(screen.getByRole("button", { name: "Reload panel" }));
    expect(screen.getByText("all fine")).toBeTruthy();
    quiet.mockRestore();
  });
});
