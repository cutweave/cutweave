// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { InspectorRow, InspectorSection } from "./InspectorSection.tsx";

afterEach(cleanup);

function Harness({
  modified,
  onReset,
  onExpand,
}: {
  modified?: boolean;
  onReset?: () => void;
  onExpand?: (v: boolean) => void;
}) {
  const [expanded, setExpanded] = useState(true);
  return h(InspectorSection, {
    label: "Transform",
    expanded,
    onExpandedChange: (v: boolean) => {
      setExpanded(v);
      onExpand?.(v);
    },
    modified,
    onReset,
    children: h(InspectorRow, {
      label: "Opacity",
      children: h("input", { "aria-label": "Opacity value" }),
    }),
  });
}

describe("InspectorSection", () => {
  it("toggle is a button with aria-expanded and aria-controls pointing at the body", () => {
    render(h(Harness));
    const toggle = screen.getByRole("button", { name: "Transform" });
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    const controls = toggle.getAttribute("aria-controls");
    expect(controls).toBeTruthy();
    expect(document.getElementById(controls as string)?.textContent).toContain("Opacity");
  });

  it("Enter and Space collapse and expand; collapsed content leaves the tree", async () => {
    const onExpand = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onExpand }));
    await user.tab();
    await user.keyboard("{Enter}");
    expect(onExpand).toHaveBeenLastCalledWith(false);
    const toggle = screen.getByRole("button", { name: "Transform" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(screen.queryByText("Opacity")).toBeNull();
    expect(screen.queryByRole("textbox")).toBeNull();
    await user.keyboard(" ");
    expect(onExpand).toHaveBeenLastCalledWith(true);
    expect(screen.getByText("Opacity")).toBeTruthy();
  });

  it("shows the section reset only when modified, named after the section", async () => {
    const onReset = vi.fn();
    const user = userEvent.setup();
    const { rerender } = render(h(Harness, { modified: false, onReset }));
    expect(screen.queryByRole("button", { name: "Reset Transform" })).toBeNull();
    rerender(h(Harness, { modified: true, onReset }));
    await user.click(screen.getByRole("button", { name: "Reset Transform" }));
    expect(onReset).toHaveBeenCalledTimes(1);
  });
});

describe("InspectorRow", () => {
  it("reset appears only while modified, so it is only then in the tab order", async () => {
    const onReset = vi.fn();
    const user = userEvent.setup();
    const { rerender } = render(
      h(InspectorRow, {
        label: "Scale",
        modified: false,
        onReset,
        children: h("input", { "aria-label": "Scale" }),
      }),
    );
    expect(screen.queryByRole("button")).toBeNull();
    rerender(
      h(InspectorRow, {
        label: "Scale",
        modified: true,
        onReset,
        children: h("input", { "aria-label": "Scale" }),
      }),
    );
    await user.tab();
    await user.tab();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Reset Scale" }));
    await user.keyboard("{Enter}");
    expect(onReset).toHaveBeenCalledTimes(1);
  });

  it("marks modified rows with data-modified and names the control via htmlFor", () => {
    const { container } = render(
      h(InspectorRow, {
        label: "Rotation",
        htmlFor: "rot",
        modified: true,
        children: h("input", { id: "rot" }),
      }),
    );
    expect(container.querySelector(".cw-row")?.getAttribute("data-modified")).toBe("true");
    expect(screen.getByLabelText("Rotation")).toBeTruthy();
  });
});
