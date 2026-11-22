// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { type TabItem, TabList, TabPanel, Tabs } from "./Tabs.tsx";

afterEach(cleanup);

const TABS: TabItem[] = [
  { id: "media", label: "Media" },
  { id: "audio", label: "Audio" },
  { id: "effects", label: "Effects", disabled: true },
  { id: "ai", label: "AI Studio", icon: "sparkles" },
];

function Harness({ onChange }: { onChange?: (v: string) => void }) {
  const [value, setValue] = useState("media");
  return h(Tabs, {
    value,
    onValueChange: (v: string) => {
      setValue(v);
      onChange?.(v);
    },
    children: [
      h(TabList, { key: "l", tabs: TABS, "aria-label": "Asset browser" }),
      h(TabPanel, { key: "m", value: "media", children: "Media bin" }),
      h(TabPanel, { key: "a", value: "audio", children: "Audio bin" }),
      h(TabPanel, { key: "i", value: "ai", children: "AI queue" }),
    ],
  });
}

describe("Tabs", () => {
  it("exposes tablist / tab / tabpanel semantics with aria-selected", () => {
    render(h(Harness));
    expect(screen.getByRole("tablist", { name: "Asset browser" })).toBeTruthy();
    expect(screen.getAllByRole("tab")).toHaveLength(4);
    expect(screen.getByRole("tab", { name: "Media" }).getAttribute("aria-selected")).toBe("true");
    expect(screen.getByRole("tab", { name: "Audio" }).getAttribute("aria-selected")).toBe("false");
    const panel = screen.getByRole("tabpanel");
    expect(panel.textContent).toBe("Media bin");
    expect(panel.getAttribute("aria-labelledby")).toBe(
      screen.getByRole("tab", { name: "Media" }).id,
    );
  });

  it("uses a roving tabindex: Tab lands on the selected tab and the rest are skipped", async () => {
    const user = userEvent.setup();
    render(h(Harness));
    await user.tab();
    expect(screen.getByRole("tab", { name: "Media" }).getAttribute("tabindex")).toBe("0");
    expect(screen.getByRole("tab", { name: "Audio" }).getAttribute("tabindex")).toBe("-1");
    await user.tab();
    expect(screen.getByRole("tab", { name: /AI Studio/ })).not.toBe(document.activeElement);
    expect(screen.getByRole("tab", { name: "Audio" })).not.toBe(document.activeElement);
  });

  it("Arrow keys move focus and activate (skipping disabled), wrapping at the ends", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onChange }));
    await user.tab();
    expect(document.activeElement).toBe(screen.getByRole("tab", { name: "Media" }));
    await user.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(screen.getByRole("tab", { name: "Audio" }));
    expect(screen.getByRole("tabpanel").textContent).toBe("Audio bin");
    await user.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(screen.getByRole("tab", { name: /AI Studio/ }));
    await user.keyboard("{ArrowRight}");
    expect(document.activeElement).toBe(screen.getByRole("tab", { name: "Media" }));
    await user.keyboard("{ArrowLeft}");
    expect(document.activeElement).toBe(screen.getByRole("tab", { name: /AI Studio/ }));
    expect(onChange).toHaveBeenLastCalledWith("ai");
  });

  it("Home and End jump to the first and last enabled tab", async () => {
    const user = userEvent.setup();
    render(h(Harness));
    await user.tab();
    await user.keyboard("{End}");
    expect(document.activeElement).toBe(screen.getByRole("tab", { name: /AI Studio/ }));
    await user.keyboard("{Home}");
    expect(document.activeElement).toBe(screen.getByRole("tab", { name: "Media" }));
    expect(screen.getByRole("tab", { name: "Media" }).getAttribute("aria-selected")).toBe("true");
  });

  it("clicking selects; disabled tabs do nothing", async () => {
    const onChange = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onChange }));
    await user.click(screen.getByRole("tab", { name: "Effects" }));
    expect(onChange).not.toHaveBeenCalled();
    await user.click(screen.getByRole("tab", { name: "Audio" }));
    expect(onChange).toHaveBeenCalledWith("audio");
  });

  it("renders an optional decorative icon beside the label", () => {
    render(h(Harness));
    const tab = screen.getByRole("tab", { name: /AI Studio/ });
    expect(tab.querySelector("svg")?.getAttribute("data-icon")).toBe("sparkles");
    expect(tab.querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");
  });

  it("keeps the selected tab in view when the selection changes", async () => {
    const scrollIntoView = vi.fn();
    Element.prototype.scrollIntoView = scrollIntoView;
    const user = userEvent.setup();
    render(h(Harness));
    scrollIntoView.mockClear();
    await user.click(screen.getByRole("tab", { name: "Audio" }));
    expect(scrollIntoView).toHaveBeenCalled();
    Reflect.deleteProperty(Element.prototype, "scrollIntoView");
  });
});
