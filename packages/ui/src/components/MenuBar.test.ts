// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { MenuBar, type MenuBarMenu } from "./MenuBar.tsx";

beforeAll(() => {
  // Radix menus measure with ResizeObserver and scroll items into view; jsdom has neither.
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollIntoView ??= () => {};
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.releasePointerCapture ??= () => {};
});

afterEach(cleanup);

function menus(spies: Record<string, () => void> = {}): MenuBarMenu[] {
  return [
    {
      id: "file",
      label: "File",
      items: [
        { id: "new", label: "New project…", shortcut: "Ctrl+N", onSelect: spies.new },
        {
          kind: "submenu",
          id: "recent",
          label: "Open recent",
          items: [{ id: "a", label: "A.cutweave", onSelect: spies.recent }],
        },
        { kind: "separator", id: "s1" },
        { id: "export", label: "Export…", shortcut: "Ctrl+M", disabled: true },
        { id: "exit", label: "Exit", onSelect: spies.exit },
      ],
    },
    {
      id: "edit",
      label: "Edit",
      items: [
        { id: "undo", label: "Undo", shortcut: "Ctrl+Z", onSelect: spies.undo },
        { id: "paste", label: "Paste", shortcut: "Ctrl+V", onSelect: spies.paste },
      ],
    },
    { id: "help", label: "Help", items: [{ id: "about", label: "About", onSelect: spies.about }] },
  ];
}

/** A text field before the bar, as the editor has fields that menu commands act on. */
function page(spies: Record<string, () => void> = {}, hidden = false) {
  return h(
    "div",
    hidden ? { "aria-hidden": "true" } : null,
    h("input", { "aria-label": "Clip name", defaultValue: "interview" }),
    h(MenuBar, { "aria-label": "Application menu", menus: menus(spies) }),
  );
}

const key = (type: "keydown" | "keyup", init: KeyboardEventInit) =>
  window.dispatchEvent(new KeyboardEvent(type, { bubbles: true, cancelable: true, ...init }));

/** Alt pressed and released on its own. */
const tapAlt = () => {
  key("keydown", { key: "Alt", altKey: true });
  key("keyup", { key: "Alt" });
};

describe("MenuBar", () => {
  it("is a named menubar whose menus are menuitems with an Alt access key", () => {
    render(page());
    const bar = screen.getByRole("menubar", { name: "Application menu" });
    const triggers = screen.getAllByRole("menuitem");
    expect(bar.contains(triggers[0] ?? null)).toBe(true);
    expect(triggers.map((t) => t.textContent)).toEqual(["File", "Edit", "Help"]);
    expect(triggers.map((t) => t.getAttribute("aria-keyshortcuts"))).toEqual([
      "Alt+F",
      "Alt+E",
      "Alt+H",
    ]);
  });

  it("Alt alone focuses the first menu and underlines access letters; Alt again goes back", async () => {
    render(page());
    const field = screen.getByRole("textbox", { name: "Clip name" });
    field.focus();
    tapAlt();
    const file = screen.getByRole("menuitem", { name: "File" });
    expect(document.activeElement).toBe(file);
    await waitFor(() =>
      expect(screen.getByRole("menubar").hasAttribute("data-access-keys")).toBe(true),
    );
    tapAlt();
    expect(document.activeElement).toBe(field);
    await waitFor(() =>
      expect(screen.getByRole("menubar").hasAttribute("data-access-keys")).toBe(false),
    );
  });

  it("F10 does the same; Ctrl+Alt (AltGr) and Alt+another key do not", () => {
    render(page());
    const field = screen.getByRole("textbox", { name: "Clip name" });
    field.focus();
    key("keydown", { key: "Alt", altKey: true, ctrlKey: true });
    key("keyup", { key: "Alt", ctrlKey: true });
    expect(document.activeElement).toBe(field);
    key("keydown", { key: "Alt", altKey: true });
    key("keydown", { key: "Tab", altKey: true });
    key("keyup", { key: "Alt" });
    expect(document.activeElement).toBe(field);
    key("keydown", { key: "F10" });
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "File" }));
    key("keydown", { key: "F10" });
    expect(document.activeElement).toBe(field);
  });

  it("arrow keys move between menus; Down opens; Escape closes, a second Escape leaves", async () => {
    const user = userEvent.setup();
    render(page());
    const field = screen.getByRole("textbox", { name: "Clip name" });
    field.focus();
    tapAlt();
    await user.keyboard("{ArrowRight}");
    const edit = screen.getByRole("menuitem", { name: "Edit" });
    expect(document.activeElement).toBe(edit);
    await user.keyboard("{ArrowDown}");
    const menu = await screen.findByRole("menu", { name: "Edit" });
    expect(document.activeElement?.textContent).toContain("Undo");
    await user.keyboard("{ArrowRight}");
    expect(await screen.findByRole("menu", { name: "Help" })).toBeTruthy();
    expect(menu.isConnected).toBe(false);
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    await waitFor(() =>
      expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Help" })),
    );
    await user.keyboard("{Escape}");
    expect(document.activeElement).toBe(field);
  });

  it("Alt+letter opens that menu with its first command focused", async () => {
    render(page());
    key("keydown", { key: "e", altKey: true });
    await screen.findByRole("menu", { name: "Edit" });
    await waitFor(() => expect(document.activeElement?.textContent).toContain("Undo"));
    await waitFor(() =>
      expect(screen.getByRole("menubar").hasAttribute("data-access-keys")).toBe(true),
    );
  });

  it("a chosen command runs after the focus is back in the field it acts on", async () => {
    const user = userEvent.setup();
    let focusedWhenRun: Element | null = null;
    const paste = vi.fn(() => {
      focusedWhenRun = document.activeElement;
    });
    render(page({ paste }));
    const field = screen.getByRole("textbox", { name: "Clip name" });
    await user.click(field);
    await user.click(screen.getByRole("menuitem", { name: "Edit" }));
    await user.click(await screen.findByRole("menuitem", { name: /Paste/ }));
    expect(paste).toHaveBeenCalledTimes(1);
    expect(focusedWhenRun).toBe(field);
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    // Radix's own close handling must not pull the focus back onto the trigger.
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(document.activeElement).toBe(field);
  });

  it("with nothing focused before, a chosen command leaves the focus on the page", async () => {
    const user = userEvent.setup();
    const about = vi.fn();
    render(page({ about }));
    await user.click(screen.getByRole("menuitem", { name: "Help" }));
    await user.click(await screen.findByRole("menuitem", { name: "About" }));
    expect(about).toHaveBeenCalledTimes(1);
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(document.activeElement).toBe(document.body);
  });

  it("submenus open and run their commands; disabled commands stay visible and inert", async () => {
    const user = userEvent.setup();
    const recent = vi.fn();
    render(page({ recent }));
    await user.click(screen.getByRole("menuitem", { name: "File" }));
    const exportItem = await screen.findByRole("menuitem", { name: /Export/ });
    expect(exportItem.getAttribute("aria-disabled")).toBe("true");
    expect(exportItem.textContent).toContain("Ctrl+M");
    const sub = screen.getByRole("menuitem", { name: /Open recent/ });
    sub.focus();
    await user.keyboard("{ArrowRight}");
    await user.click(await screen.findByRole("menuitem", { name: "A.cutweave" }));
    expect(recent).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
  });

  it("the access keys do nothing while a modal layer hides the page", () => {
    render(page({}, true));
    const field = screen.getByRole("textbox", { name: "Clip name", hidden: true });
    field.focus();
    tapAlt();
    key("keydown", { key: "F10" });
    key("keydown", { key: "f", altKey: true });
    expect(document.activeElement).toBe(field);
    expect(screen.queryByRole("menu", { hidden: true })).toBeNull();
  });

  it("other letters on a focused menu are kept from the page's single-key shortcuts", () => {
    render(page());
    tapAlt();
    const prevented: boolean[] = [];
    const spy = (e: KeyboardEvent) => prevented.push(e.defaultPrevented);
    window.addEventListener("keydown", spy);
    fireEvent.keyDown(screen.getByRole("menuitem", { name: "File" }), { key: "l" });
    window.removeEventListener("keydown", spy);
    expect(prevented).toEqual([true]);
    expect(screen.queryByRole("menu")).toBeNull();
  });

  it("a letter on a focused menu opens the menu it names", async () => {
    render(page());
    tapAlt();
    fireEvent.keyDown(screen.getByRole("menuitem", { name: "File" }), { key: "h" });
    expect(await screen.findByRole("menu", { name: "Help" })).toBeTruthy();
  });
});

describe("MenuBar choices", () => {
  it("shows a check mark and menuitemcheckbox for an item with `checked`", async () => {
    const user = userEvent.setup();
    const toggle = vi.fn();
    render(
      h(MenuBar, {
        "aria-label": "Application menu",
        menus: [
          {
            id: "view",
            label: "View",
            items: [
              { id: "assets", label: "Assets", checked: true, onSelect: toggle },
              { id: "inspector", label: "Inspector", checked: false },
            ],
          },
        ],
      }),
    );
    await user.click(screen.getByRole("menuitem", { name: "View" }));
    const assets = await screen.findByRole("menuitemcheckbox", { name: "Assets" });
    expect(assets.getAttribute("aria-checked")).toBe("true");
    const inspector = screen.getByRole("menuitemcheckbox", { name: "Inspector" });
    expect(inspector.getAttribute("aria-checked")).toBe("false");
    await user.click(assets);
    expect(toggle).toHaveBeenCalledTimes(1);
  });
});
