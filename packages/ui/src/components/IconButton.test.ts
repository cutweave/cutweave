// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { IconButton, Toolbar, ToolbarSeparator } from "./IconButton.tsx";

afterEach(cleanup);

describe("IconButton", () => {
  it("takes its accessible name from label", () => {
    render(h(IconButton, { icon: "razor", label: "Razor (C)" }));
    expect(screen.getByRole("button", { name: "Razor (C)" })).toBeTruthy();
  });

  it("omits aria-pressed for plain actions and mirrors pressed for toggles", () => {
    const { rerender } = render(h(IconButton, { icon: "magnet", label: "Snapping" }));
    expect(screen.getByRole("button").hasAttribute("aria-pressed")).toBe(false);
    rerender(h(IconButton, { icon: "magnet", label: "Snapping", pressed: true }));
    expect(screen.getByRole("button").getAttribute("aria-pressed")).toBe("true");
    rerender(h(IconButton, { icon: "magnet", label: "Snapping", pressed: false }));
    expect(screen.getByRole("button").getAttribute("aria-pressed")).toBe("false");
  });

  it("toggles with Space and Enter", async () => {
    const onClick = vi.fn();
    const user = userEvent.setup();
    render(h(IconButton, { icon: "magnet", label: "Snapping", pressed: false, onClick }));
    await user.tab();
    await user.keyboard(" ");
    await user.keyboard("{Enter}");
    expect(onClick).toHaveBeenCalledTimes(2);
  });

  it("uses icon-lg by default and icon-md at sm", () => {
    render(
      h(
        "div",
        null,
        h(IconButton, { icon: "razor", label: "Large" }),
        h(IconButton, { icon: "razor", label: "Small", size: "sm" }),
      ),
    );
    const large = screen.getByRole("button", { name: "Large" });
    const small = screen.getByRole("button", { name: "Small" });
    expect(large.querySelector("svg")?.getAttribute("data-size")).toBe("lg");
    expect(small.querySelector("svg")?.getAttribute("data-size")).toBe("md");
    expect(small.className).toContain("cw-iconbtn--sm");
  });

  it("does not fire when disabled", async () => {
    const onClick = vi.fn();
    const user = userEvent.setup();
    render(h(IconButton, { icon: "razor", label: "Razor", disabled: true, onClick }));
    await user.click(screen.getByRole("button"));
    expect(onClick).not.toHaveBeenCalled();
  });
});

describe("Toolbar", () => {
  it("moves focus with arrow keys, wrapping and skipping disabled buttons", async () => {
    const user = userEvent.setup();
    render(
      h(
        Toolbar,
        { "aria-label": "Tools" },
        h(IconButton, { icon: "razor", label: "One" }),
        h(ToolbarSeparator),
        h(IconButton, { icon: "razor", label: "Two", disabled: true }),
        h(IconButton, { icon: "razor", label: "Three" }),
      ),
    );
    expect(screen.getByRole("toolbar", { name: "Tools" })).toBeTruthy();
    const one = screen.getByRole("button", { name: "One" });
    const three = screen.getByRole("button", { name: "Three" });
    one.focus();
    await user.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(three);
    await user.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(one);
    await user.keyboard("{ArrowLeft}");
    expect(document.activeElement).toBe(three);
    await user.keyboard("{Home}");
    expect(document.activeElement).toBe(one);
    await user.keyboard("{End}");
    expect(document.activeElement).toBe(three);
  });
});
