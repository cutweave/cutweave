// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Dialog } from "./Dialog.tsx";
import { FilterChip } from "./FilterChip.tsx";
import { IconButton } from "./IconButton.tsx";
import { SearchField } from "./SearchField.tsx";
import { TooltipProvider } from "./Tooltip.tsx";

afterEach(cleanup);

function Harness(props: {
  initial?: string;
  count?: string;
  onExit?: () => void;
  onArrowDown?: () => void;
}) {
  const [value, setValue] = useState(props.initial ?? "");
  return h(SearchField, {
    value,
    onChange: setValue,
    label: "Search media",
    clearLabel: "Clear search",
    count: props.count,
    onExit: props.onExit,
    onArrowDown: props.onArrowDown,
  });
}

describe("SearchField", () => {
  it("is a named search box that reports every keystroke", async () => {
    const user = userEvent.setup();
    render(h(Harness));
    const field = screen.getByRole("searchbox", { name: "Search media" }) as HTMLInputElement;
    expect(field.placeholder).toBe("Search media");
    await user.type(field, "beach");
    expect(field.value).toBe("beach");
  });

  it("shows a clear button only while there is text, and clearing keeps focus in the field", async () => {
    const user = userEvent.setup();
    render(h(Harness));
    expect(screen.queryByRole("button", { name: "Clear search" })).toBeNull();
    const field = screen.getByRole("searchbox", { name: "Search media" }) as HTMLInputElement;
    await user.type(field, "x");
    await user.click(screen.getByRole("button", { name: "Clear search" }));
    expect(field.value).toBe("");
    expect(document.activeElement).toBe(field);
    expect(screen.queryByRole("button", { name: "Clear search" })).toBeNull();
  });

  it("clears on the first Esc and leaves on the second", async () => {
    const user = userEvent.setup();
    const onExit = vi.fn();
    render(h(Harness, { initial: "abc", onExit }));
    const field = screen.getByRole("searchbox") as HTMLInputElement;
    field.focus();
    await user.keyboard("{Escape}");
    expect(field.value).toBe("");
    expect(onExit).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(field);
    await user.keyboard("{Escape}");
    expect(onExit).toHaveBeenCalledTimes(1);
  });

  it("still clears on Esc inside a dialog that keeps itself open (Radix prevents the default first)", async () => {
    const user = userEvent.setup();
    const onOpenChange = vi.fn();
    const onExit = vi.fn();
    render(
      h(
        TooltipProvider,
        null,
        h(Dialog, {
          open: true,
          onOpenChange,
          title: "Settings",
          // The dialog holds Esc while the field has focus (its own guard prevents the default).
          dirty: true,
          children: h(Harness, { initial: "abc", onExit }),
        }),
      ),
    );
    const field = screen.getByRole("searchbox") as HTMLInputElement;
    field.focus();
    await user.keyboard("{Escape}");
    expect(field.value).toBe("");
    expect(onOpenChange).not.toHaveBeenCalled();
    await user.keyboard("{Escape}");
    expect(onExit).toHaveBeenCalledTimes(1);
    expect(onOpenChange).not.toHaveBeenCalled();
  });

  it("blurs on Esc of an empty field when the host gives no exit", async () => {
    const user = userEvent.setup();
    render(h(Harness));
    const field = screen.getByRole("searchbox") as HTMLInputElement;
    field.focus();
    await user.keyboard("{Escape}");
    expect(document.activeElement).not.toBe(field);
  });

  it("hands Arrow Down to the host", async () => {
    const user = userEvent.setup();
    const onArrowDown = vi.fn();
    render(h(Harness, { onArrowDown }));
    (screen.getByRole("searchbox") as HTMLInputElement).focus();
    await user.keyboard("{ArrowDown}");
    expect(onArrowDown).toHaveBeenCalledTimes(1);
  });

  it("announces the result count while there is text", async () => {
    const user = userEvent.setup();
    render(h(Harness, { count: "12 of 340" }));
    expect(screen.queryByRole("status")).toBeNull();
    await user.type(screen.getByRole("searchbox"), "a");
    expect(screen.getByRole("status").textContent).toBe("12 of 340");
  });
});

describe("FilterChip", () => {
  it("removes its filter", async () => {
    const user = userEvent.setup();
    const onRemove = vi.fn();
    render(h(FilterChip, { label: "Audio", removeLabel: "Remove filter Audio", onRemove }));
    expect(screen.getByText("Audio")).toBeTruthy();
    await user.click(screen.getByRole("button", { name: "Remove filter Audio" }));
    expect(onRemove).toHaveBeenCalledTimes(1);
  });
});

describe("IconButton badge", () => {
  it("shows a count from 1 and keeps the accessible name", () => {
    render(h(IconButton, { icon: "sliders-horizontal", label: "Filters, 2 active", badge: 2 }));
    const button = screen.getByRole("button", { name: "Filters, 2 active" });
    expect(button.querySelector(".cw-iconbtn__badge")?.textContent).toBe("2");
  });

  it("shows nothing for zero and caps large counts", () => {
    const { rerender } = render(h(IconButton, { icon: "x", label: "Filters", badge: 0 }));
    expect(document.querySelector(".cw-iconbtn__badge")).toBeNull();
    rerender(h(IconButton, { icon: "x", label: "Filters", badge: 12 }));
    expect(document.querySelector(".cw-iconbtn__badge")?.textContent).toBe("9+");
  });
});
