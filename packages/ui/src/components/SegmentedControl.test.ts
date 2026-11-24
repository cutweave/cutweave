// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SegmentedControl, type SegmentedOption } from "./SegmentedControl.tsx";

afterEach(cleanup);

const OPTIONS: SegmentedOption[] = [
  { value: "16:9", label: "16:9" },
  { value: "9:16", label: "9:16" },
  { value: "1:1", label: "1:1", disabled: true },
  { value: "4:5", label: "4:5" },
];

function Harness({ onChange }: { onChange: (v: string) => void }) {
  const [value, setValue] = useState("16:9");
  return h(SegmentedControl, {
    options: OPTIONS,
    value,
    "aria-label": "Aspect ratio",
    onChange: (v: string) => {
      setValue(v);
      onChange(v);
    },
  });
}

describe("SegmentedControl", () => {
  it("selects none when the value is null", () => {
    render(
      h(SegmentedControl, {
        options: OPTIONS,
        value: null,
        "aria-label": "Aspect ratio",
        onChange: () => {},
      }),
    );
    for (const radio of screen.getAllByRole("radio")) {
      expect(radio.getAttribute("aria-checked")).toBe("false");
    }
  });

  it("is a named radiogroup of radios with the selected one checked", () => {
    render(h(Harness, { onChange: () => {} }));
    expect(screen.getByRole("radiogroup", { name: "Aspect ratio" })).toBeTruthy();
    expect(screen.getAllByRole("radio")).toHaveLength(4);
    expect(screen.getByRole("radio", { name: "16:9" }).getAttribute("aria-checked")).toBe("true");
    expect(screen.getByRole("radio", { name: "9:16" }).getAttribute("aria-checked")).toBe("false");
  });

  it("selects on click and never deselects by clicking the active item", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onChange }));
    await user.click(screen.getByRole("radio", { name: "9:16" }));
    expect(onChange).toHaveBeenCalledExactlyOnceWith("9:16");
    await user.click(screen.getByRole("radio", { name: "9:16" }));
    expect(onChange).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("radio", { name: "9:16" }).getAttribute("aria-checked")).toBe("true");
  });

  it("makes Arrow keys move and select, skipping disabled items", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onChange }));
    await user.tab();
    expect(document.activeElement).toBe(screen.getByRole("radio", { name: "16:9" }));
    await user.keyboard("{ArrowRight}");
    expect(onChange).toHaveBeenLastCalledWith("9:16");
    await user.keyboard("{ArrowRight}");
    expect(onChange).toHaveBeenLastCalledWith("4:5");
    expect(screen.getByRole("radio", { name: "4:5" }).getAttribute("aria-checked")).toBe("true");
    await user.keyboard("{ArrowLeft}");
    expect(onChange).toHaveBeenLastCalledWith("9:16");
  });

  it("keeps a disabled item visible but unselectable", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onChange }));
    const disabled = screen.getByRole("radio", { name: "1:1" });
    expect((disabled as HTMLButtonElement).disabled).toBe(true);
    await user.click(disabled);
    expect(onChange).not.toHaveBeenCalled();
  });
});

describe("SegmentedControl iconOnly", () => {
  const WORKSPACES: SegmentedOption[] = [
    { value: "short", label: "Short-form", icon: "smartphone" },
    { value: "long", label: "Long-form", icon: "monitor" },
  ];

  it("shows icons only, keeps each label as the radio's name and as a tooltip", async () => {
    const onChange = vi.fn();
    render(
      h(SegmentedControl, {
        options: WORKSPACES,
        value: "short",
        "aria-label": "Workspace",
        iconOnly: true,
        onChange,
      }),
    );
    const long = screen.getByRole("radio", { name: "Long-form" });
    expect(long.textContent).toBe("");
    expect(long.querySelector("svg")).not.toBeNull();
    const user = userEvent.setup();
    await user.hover(long);
    expect(await screen.findByRole("tooltip", { name: "Long-form" })).toBeTruthy();
    await user.click(long);
    expect(onChange).toHaveBeenCalledWith("long");
  });

  it("without iconOnly shows the icon next to the label", () => {
    render(
      h(SegmentedControl, {
        options: WORKSPACES,
        value: "short",
        "aria-label": "Workspace",
        onChange: () => {},
      }),
    );
    const short = screen.getByRole("radio", { name: "Short-form" });
    expect(short.textContent).toBe("Short-form");
    expect(short.querySelector("svg")).not.toBeNull();
  });
});
