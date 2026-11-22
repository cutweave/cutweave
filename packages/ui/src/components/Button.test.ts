// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Button } from "./Button.tsx";

afterEach(cleanup);

describe("Button", () => {
  it("is a native button of type button that activates with Enter and Space", async () => {
    const onClick = vi.fn();
    const user = userEvent.setup();
    render(h(Button, { onClick }, "Export video"));
    const button = screen.getByRole("button", { name: "Export video" });
    expect(button.tagName).toBe("BUTTON");
    expect(button).toHaveProperty("type", "button");
    await user.tab();
    expect(button).toBe(document.activeElement);
    await user.keyboard("{Enter}");
    await user.keyboard(" ");
    expect(onClick).toHaveBeenCalledTimes(2);
  });

  it("maps variant and size to classes", () => {
    render(h(Button, { variant: "primary", size: "lg" }, "Generate clip"));
    const button = screen.getByRole("button");
    expect(button.className).toContain("cw-btn--primary");
    expect(button.className).toContain("cw-btn--lg");
    expect(button.className).toContain("body-strong");
  });

  it("uses label type at sm and no modifier classes for the default", () => {
    render(h("div", null, h(Button, { size: "sm" }, "Small"), h(Button, null, "Default")));
    const small = screen.getByRole("button", { name: "Small" });
    expect(small.className).toContain("cw-btn--sm");
    expect(small.className).toContain("label");
    expect(screen.getByRole("button", { name: "Default" }).className).toBe("cw-btn body-strong");
  });

  it("renders a decorative leading icon", () => {
    render(h(Button, { icon: "plus" }, "Add to timeline"));
    const icon = screen.getByRole("button").querySelector("svg");
    expect(icon?.getAttribute("aria-hidden")).toBe("true");
  });

  it("is skipped by Tab and inert when disabled, keeping its label", async () => {
    const onClick = vi.fn();
    const user = userEvent.setup();
    render(h(Button, { disabled: true, onClick }, "Export video"));
    await user.tab();
    await user.click(screen.getByText("Export video"));
    expect(document.activeElement).toBe(document.body);
    expect(onClick).not.toHaveBeenCalled();
  });
});
