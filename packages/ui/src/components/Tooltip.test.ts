// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { IconButton } from "./IconButton.tsx";
import { Menu } from "./Menu.tsx";
import { Tooltip } from "./Tooltip.tsx";

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
});

afterEach(cleanup);

function demo(props: { keys?: string[]; delayDuration?: number } = {}) {
  return h(Tooltip, {
    label: "Split at playhead",
    delayDuration: 0,
    ...props,
    children: h(IconButton, { icon: "scissors", label: "Split" }),
  });
}

describe("Tooltip", () => {
  it("is hidden until hover or focus", () => {
    render(demo());
    expect(screen.queryByRole("tooltip")).toBeNull();
  });

  it("shows on keyboard focus, wired with aria-describedby, and the anchor keeps its own name", async () => {
    const user = userEvent.setup();
    render(demo());
    await user.tab();
    const tip = await screen.findByRole("tooltip");
    expect(tip.textContent).toContain("Split at playhead");
    const anchor = screen.getByRole("button", { name: "Split" });
    expect(anchor.getAttribute("aria-describedby")).toBe(tip.id);
  });

  it("shows on hover", async () => {
    const user = userEvent.setup();
    render(demo());
    await user.hover(screen.getByRole("button", { name: "Split" }));
    expect(await screen.findByRole("tooltip")).toBeTruthy();
  });

  it("renders each shortcut key as a mono key cap", async () => {
    const user = userEvent.setup();
    render(demo({ keys: ["Shift", "Backspace"] }));
    await user.tab();
    await screen.findByRole("tooltip");
    const caps = Array.from(document.querySelectorAll("kbd.cw-kbd")).map((k) => k.textContent);
    expect(caps).toContain("Shift");
    expect(caps).toContain("Backspace");
  });

  it("Escape hides it and keeps focus on the anchor", async () => {
    const user = userEvent.setup();
    render(demo());
    await user.tab();
    await screen.findByRole("tooltip");
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("tooltip")).toBeNull());
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Split" }));
  });

  it("blur hides it", async () => {
    const user = userEvent.setup();
    render(demo());
    await user.tab();
    await screen.findByRole("tooltip");
    await user.tab();
    await waitFor(() => expect(screen.queryByRole("tooltip")).toBeNull());
  });

  it("holds no interactive content", async () => {
    const user = userEvent.setup();
    render(demo({ keys: ["S"] }));
    await user.tab();
    const tip = await screen.findByRole("tooltip");
    expect(tip.querySelector("button, a, input, [tabindex]")).toBeNull();
  });
});

describe("Tooltip as a menu trigger", () => {
  it("lets a dropdown Menu open from a button that has a tooltip", async () => {
    Element.prototype.scrollIntoView ??= () => {};
    Element.prototype.hasPointerCapture ??= () => false;
    Element.prototype.releasePointerCapture ??= () => {};
    const user = userEvent.setup();
    render(
      h(Menu, {
        mode: "dropdown",
        "aria-label": "Sort media",
        items: [{ id: "name", label: "Name", checked: false, choice: "radio" }],
        children: h(Tooltip, {
          label: "Sort",
          children: h(IconButton, { icon: "arrow-up-down", label: "Sort" }),
        }),
      }),
    );
    await user.click(screen.getByRole("button", { name: "Sort" }));
    expect(await screen.findByRole("menuitemradio", { name: "Name" })).toBeTruthy();
  });
});
