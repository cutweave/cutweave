// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { Menu, type MenuEntry } from "./Menu.tsx";

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

function entries(spies: Record<string, () => void> = {}): MenuEntry[] {
  return [
    {
      id: "split",
      label: "Split at playhead",
      icon: "scissors",
      shortcut: "S",
      onSelect: spies.split,
    },
    { id: "ripple", label: "Ripple delete", shortcut: "Shift+Backspace", onSelect: spies.ripple },
    { id: "paste", label: "Paste attributes", shortcut: "Ctrl+Shift+V", disabled: true },
    { id: "replace", label: "Replace clip", onSelect: spies.replace },
    { kind: "separator", id: "s1" },
    { id: "ask", label: "Ask agent…", variant: "agent", onSelect: spies.ask },
    { kind: "separator", id: "s2" },
    {
      id: "delete",
      label: "Delete",
      icon: "trash-2",
      variant: "danger",
      shortcut: "Backspace",
      onSelect: spies.del,
    },
  ];
}

function dropdown(spies: Record<string, () => void> = {}) {
  return h(
    "div",
    null,
    h("button", { type: "button", id: "outside" }, "Outside"),
    h(Menu, {
      mode: "dropdown",
      items: entries(spies),
      "aria-label": "Clip actions",
      children: h("button", { type: "button" }, "More"),
    }),
  );
}

describe("Menu (dropdown)", () => {
  it("opens from the keyboard with role=menu, named, and focuses the menu", async () => {
    const user = userEvent.setup();
    render(dropdown());
    await user.tab();
    await user.tab();
    await user.keyboard("{Enter}");
    const menu = await screen.findByRole("menu", { name: "Clip actions" });
    expect(menu).toBeTruthy();
    expect(screen.getAllByRole("menuitem")).toHaveLength(6);
    expect(menu.contains(document.activeElement)).toBe(true);
  });

  it("shows shortcuts as real text and separators as role=separator", async () => {
    const user = userEvent.setup();
    render(dropdown());
    await user.click(screen.getByRole("button", { name: "More" }));
    const split = await screen.findByRole("menuitem", { name: /Split at playhead/ });
    expect(split.textContent).toContain("S");
    expect(screen.getByRole("menuitem", { name: /Ripple delete/ }).textContent).toContain(
      "Shift+Backspace",
    );
    expect(screen.getAllByRole("separator")).toHaveLength(2);
  });

  it("Arrow keys move through items, skipping disabled; Home/End jump; focus loops", async () => {
    const user = userEvent.setup();
    render(dropdown());
    await user.click(screen.getByRole("button", { name: "More" }));
    await screen.findByRole("menu");
    await user.keyboard("{ArrowDown}");
    expect(document.activeElement?.textContent).toContain("Split at playhead");
    await user.keyboard("{ArrowDown}");
    expect(document.activeElement?.textContent).toContain("Ripple delete");
    await user.keyboard("{ArrowDown}");
    expect(document.activeElement?.textContent).toContain("Replace clip");
    await user.keyboard("{ArrowUp}");
    expect(document.activeElement?.textContent).toContain("Ripple delete");
    await user.keyboard("{End}");
    expect(document.activeElement?.textContent).toContain("Delete");
    await user.keyboard("{ArrowDown}");
    expect(document.activeElement?.textContent).toContain("Split at playhead");
    await user.keyboard("{Home}");
    expect(document.activeElement?.textContent).toContain("Split at playhead");
  });

  it("marks disabled items aria-disabled and does not run them", async () => {
    const user = userEvent.setup();
    render(dropdown());
    await user.click(screen.getByRole("button", { name: "More" }));
    const paste = await screen.findByRole("menuitem", { name: /Paste attributes/ });
    expect(paste.getAttribute("aria-disabled")).toBe("true");
    expect(paste.hasAttribute("data-disabled")).toBe(true);
  });

  it("Enter activates the focused item, closes the menu and restores focus to the anchor", async () => {
    const split = vi.fn();
    const user = userEvent.setup();
    render(dropdown({ split }));
    const trigger = screen.getByRole("button", { name: "More" });
    await user.click(trigger);
    await screen.findByRole("menu");
    await user.keyboard("{ArrowDown}{Enter}");
    expect(split).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    expect(document.activeElement).toBe(trigger);
  });

  it("Escape closes without running anything and returns focus to the anchor", async () => {
    const split = vi.fn();
    const user = userEvent.setup();
    render(dropdown({ split }));
    const trigger = screen.getByRole("button", { name: "More" });
    trigger.focus();
    await user.keyboard("{Enter}");
    await screen.findByRole("menu");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    expect(split).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(trigger);
  });

  it("traps focus: Tab does not leave the open menu", async () => {
    const user = userEvent.setup();
    render(dropdown());
    await user.click(screen.getByRole("button", { name: "More" }));
    const menu = await screen.findByRole("menu");
    await user.keyboard("{ArrowDown}");
    await user.tab();
    await user.tab();
    expect(menu.contains(document.activeElement)).toBe(true);
    expect(document.activeElement).not.toBe(document.getElementById("outside"));
  });

  it("type-ahead focuses the item whose label starts with the typed letters", async () => {
    const user = userEvent.setup();
    render(dropdown());
    await user.click(screen.getByRole("button", { name: "More" }));
    await screen.findByRole("menu");
    await user.keyboard("re");
    expect(document.activeElement?.textContent).toContain("Replace clip");
  });

  it("Ask agent has sparkles and words; Delete is danger with words and an icon", async () => {
    const user = userEvent.setup();
    render(dropdown());
    await user.click(screen.getByRole("button", { name: "More" }));
    const ask = await screen.findByRole("menuitem", { name: /Ask agent/ });
    expect(ask.className).toContain("cw-menu__item--agent");
    expect(ask.querySelector("svg")?.getAttribute("data-icon")).toBe("sparkles");
    const del = screen.getByRole("menuitem", { name: /Delete/ });
    expect(del.className).toContain("cw-menu__item--danger");
    expect(del.querySelector("svg")?.getAttribute("data-icon")).toBe("trash-2");
  });

  it("clicking an item runs its command", async () => {
    const ask = vi.fn();
    const user = userEvent.setup();
    render(dropdown({ ask }));
    await user.click(screen.getByRole("button", { name: "More" }));
    await user.click(await screen.findByRole("menuitem", { name: /Ask agent/ }));
    expect(ask).toHaveBeenCalledTimes(1);
  });
});

describe("Menu (context)", () => {
  it("opens on contextmenu over the anchor and closes with Escape", async () => {
    const user = userEvent.setup();
    render(
      h(Menu, {
        items: entries(),
        "aria-label": "Clip actions for interview_A.mov",
        children: h("div", { "data-testid": "clip", tabIndex: 0 }, "interview_A.mov"),
      }),
    );
    expect(screen.queryByRole("menu")).toBeNull();
    fireEvent.contextMenu(screen.getByTestId("clip"));
    const menu = await screen.findByRole("menu", { name: "Clip actions for interview_A.mov" });
    expect(menu).toBeTruthy();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
  });

  it("runs the chosen command from the keyboard", async () => {
    const ripple = vi.fn();
    const user = userEvent.setup();
    render(
      h(Menu, {
        items: entries({ ripple }),
        "aria-label": "Clip actions",
        children: h("div", { "data-testid": "clip", tabIndex: 0 }, "clip"),
      }),
    );
    fireEvent.contextMenu(screen.getByTestId("clip"));
    await screen.findByRole("menu");
    await user.keyboard("{ArrowDown}{ArrowDown}{Enter}");
    expect(ripple).toHaveBeenCalledTimes(1);
  });
});
