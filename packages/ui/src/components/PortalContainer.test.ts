// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-2H.15: floating layers open in the window their panel is in. A second (iframe) window stands
// in for a detached panel's window.
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { createElement as h } from "react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { Dialog } from "./Dialog.tsx";
import { Menu, type MenuEntry } from "./Menu.tsx";
import { PortalContainerProvider, useOwnerWindow } from "./PortalContainer.ts";
import { Tooltip } from "./Tooltip.tsx";

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollIntoView ??= () => {};
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
});

afterEach(() => {
  cleanup();
  for (const frame of document.querySelectorAll("iframe")) frame.remove();
});

/** A second window, as a detached panel's. */
function secondWindow(): Window {
  const frame = document.createElement("iframe");
  document.body.append(frame);
  const win = frame.contentWindow;
  if (win === null) throw new Error("no iframe window");
  return win;
}

const items: MenuEntry[] = [{ id: "split", label: "Split at playhead" }];

describe("PortalContainerProvider", () => {
  it("opens a point menu, with its anchor, in the container's window", () => {
    const other = secondWindow();
    render(
      h(
        PortalContainerProvider,
        { container: other.document.body },
        h(Menu, {
          mode: "point",
          open: true,
          anchor: { x: 40, y: 50 },
          items,
          "aria-label": "Clip actions",
        }),
      ),
    );
    const menu = other.document.querySelector('[role="menu"]');
    expect(menu?.getAttribute("aria-label")).toBe("Clip actions");
    expect(menu?.textContent).toContain("Split at playhead");
    expect(other.document.querySelector(".cw-menu__anchor")).not.toBeNull();
    expect(document.querySelector('[role="menu"]')).toBeNull();
    expect(document.querySelector(".cw-menu__anchor")).toBeNull();
  });

  it("leaves a menu in the main document without a provider", () => {
    render(
      h(Menu, { mode: "point", open: true, anchor: { x: 1, y: 1 }, items, "aria-label": "M" }),
    );
    expect(document.querySelector('[role="menu"]')).not.toBeNull();
  });

  it("shows a tooltip in the container's window", async () => {
    const other = secondWindow();
    render(
      h(
        PortalContainerProvider,
        { container: other.document.body },
        h(Tooltip, {
          label: "Keep on top",
          children: h("button", { type: "button" }, "pin"),
        }),
      ),
    );
    await act(async () => {
      fireEvent.focus(screen.getByRole("button", { name: "pin" }));
    });
    expect(other.document.body.textContent).toContain("Keep on top");
    expect(document.querySelector(".cw-tooltip")).toBeNull();
  });

  it("opens a dialog in the main document unless it is given a container", () => {
    const other = secondWindow();
    const { rerender } = render(
      h(
        PortalContainerProvider,
        { container: other.document.body },
        h(Dialog, { open: true, onOpenChange: () => {}, title: "Settings", children: "body" }),
      ),
    );
    // An app dialog stays in the main window, even when a detached panel raised it.
    expect(document.querySelector('[role="dialog"]')?.textContent).toContain("Settings");
    expect(other.document.querySelector('[role="dialog"]')).toBeNull();
    rerender(
      h(Dialog, {
        open: true,
        onOpenChange: () => {},
        title: "Command palette",
        container: other.document.body,
        children: h("input", { "aria-label": "Search" }),
      }),
    );
    const dialog = other.document.querySelector('[role="dialog"]');
    expect(dialog?.textContent).toContain("Command palette");
    // It focuses its first field in that window.
    expect(other.document.activeElement?.getAttribute("aria-label")).toBe("Search");
  });

  it("keeps Tab inside a dialog that is open in another window", () => {
    const other = secondWindow();
    render(
      h(Dialog, {
        open: true,
        onOpenChange: () => {},
        title: "Command palette",
        container: other.document.body,
        children: [
          h("input", { key: "a", "aria-label": "Search" }),
          h("button", { key: "b", type: "button", "aria-label": "Last" }),
        ],
      }),
    );
    const search = other.document.querySelector<HTMLElement>('[aria-label="Search"]');
    const last = other.document.querySelector<HTMLElement>('[aria-label="Last"]');
    expect(other.document.activeElement).toBe(search);
    // Tab from the last field wraps to the first; Shift+Tab from the first wraps to the last.
    last?.focus();
    fireEvent.keyDown(last as HTMLElement, { key: "Tab" });
    expect(other.document.activeElement).toBe(search);
    fireEvent.keyDown(search as HTMLElement, { key: "Tab", shiftKey: true });
    expect(other.document.activeElement).toBe(last);
    // In between, Tab moves on by one and stays inside.
    search?.focus();
    fireEvent.keyDown(search as HTMLElement, { key: "Tab" });
    expect(other.document.activeElement).toBe(last);
  });

  it("names the window a part of the tree is shown in", () => {
    const other = secondWindow();
    const seen: (Window | null)[] = [];
    function Probe() {
      seen.push(useOwnerWindow());
      return null;
    }
    render(h(PortalContainerProvider, { container: other.document.body }, h(Probe)));
    render(h(Probe));
    expect(seen[0]).toBe(other);
    expect(seen[1]).toBe(globalThis.window);
  });
});
