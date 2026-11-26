// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { type LayoutOption, LayoutPicker } from "./LayoutPicker.tsx";

afterEach(cleanup);

const OPTIONS: LayoutOption[] = [
  {
    id: "side_by_side",
    label: "Side by side",
    cells: [
      { x: 0, y: 0, w: 0.5, h: 1 },
      { x: 0.5, y: 0, w: 0.5, h: 1 },
    ],
  },
  {
    id: "top_bottom",
    label: "Top and bottom",
    cells: [
      { x: 0, y: 0, w: 1, h: 0.5 },
      { x: 0, y: 0.5, w: 1, h: 0.5 },
    ],
  },
];

describe("LayoutPicker", () => {
  it("is a named group of one button per layout, named by its label", () => {
    render(h(LayoutPicker, { options: OPTIONS, onPick: () => {}, "aria-label": "Layout" }));
    expect(screen.getByRole("group", { name: "Layout" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Side by side" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Top and bottom" })).toBeTruthy();
  });

  it("draws one rectangle per cell, the first in the accent", () => {
    render(h(LayoutPicker, { options: OPTIONS, onPick: () => {}, "aria-label": "Layout" }));
    const button = screen.getByRole("button", { name: "Side by side" });
    expect(button.querySelectorAll(".cw-layoutpicker__cell")).toHaveLength(2);
    expect(button.querySelectorAll(".cw-layoutpicker__cell--first")).toHaveLength(1);
  });

  it("picks with a click and with the keyboard, and not when disabled", async () => {
    const onPick = vi.fn();
    const user = userEvent.setup();
    const { rerender } = render(
      h(LayoutPicker, { options: OPTIONS, onPick, "aria-label": "Layout" }),
    );
    await user.click(screen.getByRole("button", { name: "Top and bottom" }));
    await user.tab();
    await user.keyboard("{Enter}");
    expect(onPick.mock.calls.map((c) => c[0])).toEqual(["top_bottom", "side_by_side"]);
    rerender(h(LayoutPicker, { options: OPTIONS, onPick, disabled: true, "aria-label": "Layout" }));
    await user.click(screen.getByRole("button", { name: "Side by side" }));
    expect(onPick).toHaveBeenCalledTimes(2);
  });
});
