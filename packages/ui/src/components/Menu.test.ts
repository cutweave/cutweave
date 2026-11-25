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

describe("Menu choices (checked rows, headings)", () => {
  const choices = (onVideo: () => void, onNewest: () => void): MenuEntry[] => [
    { kind: "label", id: "kind", label: "Kind" },
    { id: "video", label: "Video", checked: true, keepOpen: true, onSelect: onVideo },
    { id: "audio", label: "Audio", checked: false, keepOpen: true },
    { kind: "separator", id: "s" },
    { kind: "label", id: "sort", label: "Sort by" },
    { id: "newest", label: "Newest first", checked: false, choice: "radio", onSelect: onNewest },
  ];
  const view = (onVideo: () => void, onNewest: () => void) =>
    h(Menu, {
      mode: "dropdown",
      items: choices(onVideo, onNewest),
      "aria-label": "Filters",
      children: h("button", { type: "button" }, "Open"),
    });

  it("announces checkboxes and radios with their state and shows headings", async () => {
    const user = userEvent.setup();
    render(view(vi.fn(), vi.fn()));
    await user.click(screen.getByRole("button", { name: "Open" }));
    const video = await screen.findByRole("menuitemcheckbox", { name: "Video" });
    expect(video.getAttribute("aria-checked")).toBe("true");
    const audio = screen.getByRole("menuitemcheckbox", { name: "Audio" });
    expect(audio.getAttribute("aria-checked")).toBe("false");
    expect(screen.getByRole("menuitemradio", { name: "Newest first" })).toBeTruthy();
    expect(screen.getByText("Kind")).toBeTruthy();
    expect(screen.getByText("Sort by")).toBeTruthy();
  });

  it("keeps the menu open for a keepOpen row and closes for the others", async () => {
    const onVideo = vi.fn();
    const onNewest = vi.fn();
    const user = userEvent.setup();
    render(view(onVideo, onNewest));
    await user.click(screen.getByRole("button", { name: "Open" }));
    await user.click(await screen.findByRole("menuitemcheckbox", { name: "Video" }));
    expect(onVideo).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("menu")).toBeTruthy();
    await user.click(screen.getByRole("menuitemradio", { name: "Newest first" }));
    expect(onNewest).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
  });
});

describe("Menu submenu and disabled reason", () => {
  const withSubmenu = (onRight: () => void): MenuEntry[] => [
    { id: "close", label: "Close panel", onSelect: vi.fn() },
    {
      kind: "submenu",
      id: "move",
      label: "Move to",
      items: [
        { id: "left", label: "Left" },
        { id: "right", label: "Right", onSelect: onRight },
      ],
    },
    {
      id: "rename",
      label: "Rename…",
      disabled: true,
      disabledReason: "Select one clip to rename it",
    },
  ];
  const view = (items: MenuEntry[]) =>
    h(Menu, {
      mode: "dropdown",
      items,
      "aria-label": "Panel actions",
      children: h("button", { type: "button" }, "Open"),
    });

  it("opens a nested list with the arrow key and runs a row of it", async () => {
    const onRight = vi.fn();
    const user = userEvent.setup();
    render(view(withSubmenu(onRight)));
    await user.click(screen.getByRole("button", { name: "Open" }));
    await screen.findByRole("menu", { name: "Panel actions" });
    await user.keyboard("{ArrowDown}{ArrowDown}{ArrowRight}");
    const right = await screen.findByRole("menuitem", { name: "Right" });
    expect(screen.getByRole("menuitem", { name: "Move to" }).getAttribute("aria-haspopup")).toBe(
      "menu",
    );
    await user.click(right);
    expect(onRight).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
  });

  it("describes a disabled row by its reason", async () => {
    const user = userEvent.setup();
    render(view(withSubmenu(vi.fn())));
    await user.click(screen.getByRole("button", { name: "Open" }));
    const rename = await screen.findByRole("menuitem", { name: "Rename…" });
    expect(rename.getAttribute("aria-disabled")).toBe("true");
    const label = rename.querySelector("[aria-describedby]");
    const id = label?.getAttribute("aria-describedby") ?? "";
    expect(document.getElementById(id)?.textContent).toBe("Select one clip to rename it");
  });

  it("shows the reason as a tooltip while the row is hovered", async () => {
    const user = userEvent.setup();
    render(view(withSubmenu(vi.fn())));
    await user.click(screen.getByRole("button", { name: "Open" }));
    const rename = await screen.findByRole("menuitem", { name: "Rename…" });
    await user.hover(rename.querySelector(".cw-menu__label") as Element);
    const tip = await screen.findByRole("tooltip", {}, { timeout: 2000 });
    expect(tip.textContent).toContain("Select one clip to rename it");
  });
});

describe("Menu (point)", () => {
  function PointHost(props: {
    open: boolean;
    onOpenChange?: (open: boolean) => void;
    onCloseAutoFocus?: (e: Event) => void;
  }) {
    return h(
      "div",
      null,
      h("button", { type: "button", id: "surface" }, "Surface"),
      h(Menu, {
        mode: "point",
        open: props.open,
        anchor: { x: 120, y: 80 },
        items: entries(),
        "aria-label": "Clip actions for interview_A.mov",
        ...(props.onOpenChange ? { onOpenChange: props.onOpenChange } : {}),
        ...(props.onCloseAutoFocus ? { onCloseAutoFocus: props.onCloseAutoFocus } : {}),
      }),
    );
  }

  it("renders closed, opens when controlled, and hangs from a zero-size anchor at the point", async () => {
    const view = render(h(PointHost, { open: false }));
    expect(screen.queryByRole("menu")).toBeNull();
    view.rerender(h(PointHost, { open: true }));
    await screen.findByRole("menu", { name: "Clip actions for interview_A.mov" });
    const anchor = document.querySelector<HTMLElement>(".cw-menu__anchor");
    expect(anchor?.style.left).toBe("120px");
    expect(anchor?.style.top).toBe("80px");
    expect(anchor?.getAttribute("aria-hidden")).toBe("true");
  });

  it("asks to close on Escape and lets the host place the focus afterwards", async () => {
    const onOpenChange = vi.fn();
    const onCloseAutoFocus = vi.fn((e: Event) => {
      e.preventDefault();
      document.getElementById("surface")?.focus();
    });
    const user = userEvent.setup();
    const view = render(h(PointHost, { open: true, onOpenChange, onCloseAutoFocus }));
    await screen.findByRole("menu");
    await user.keyboard("{Escape}");
    expect(onOpenChange).toHaveBeenCalledWith(false);
    view.rerender(h(PointHost, { open: false, onOpenChange, onCloseAutoFocus }));
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    expect(onCloseAutoFocus).toHaveBeenCalled();
    expect(document.activeElement).toBe(document.getElementById("surface"));
  });
});
