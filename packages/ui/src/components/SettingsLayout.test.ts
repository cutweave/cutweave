// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SearchField } from "./SearchField.tsx";
import { SettingRow } from "./SettingRow.tsx";
import { SettingsLayout, SettingsResultGroup } from "./SettingsLayout.tsx";
import { Switch } from "./Switch.tsx";
import { TooltipProvider } from "./Tooltip.tsx";

afterEach(cleanup);

const GROUPS = [
  {
    id: "prefs",
    label: "Preferences",
    pages: [
      { id: "general", label: "General", icon: "settings" as const },
      { id: "appearance", label: "Appearance", icon: "palette" as const },
    ],
  },
  { id: "adv", pages: [{ id: "advanced", label: "Advanced" }] },
];

function Harness({ onSelect = () => {} }: { onSelect?: (id: string) => void }) {
  const [active, setActive] = useState<string | null>("general");
  const [query, setQuery] = useState("");
  return h(
    TooltipProvider,
    null,
    h(SettingsLayout, {
      navLabel: "Settings pages",
      search: h(SearchField, {
        value: query,
        onChange: (value: string) => {
          setQuery(value);
          setActive(value === "" ? "general" : null);
        },
        label: "Search settings",
        clearLabel: "Clear search",
      }),
      groups: GROUPS,
      activeId: active,
      onSelect: (id: string) => {
        setActive(id);
        onSelect(id);
      },
      heading: active === null ? "Results" : (active[0]?.toUpperCase() ?? "") + active.slice(1),
      resultsLabel: "Search results",
      children:
        active === null
          ? h(SettingsResultGroup, {
              label: "General",
              children: h("p", null, "a result"),
            })
          : h("p", null, `rows of ${active}`),
    }),
  );
}

describe("SettingsLayout", () => {
  it("is a vertical tablist of pages, with the open one selected and its panel shown", () => {
    render(h(Harness));
    const list = screen.getByRole("tablist", { name: "Settings pages" });
    expect(list.getAttribute("aria-orientation")).toBe("vertical");
    const tabs = within(list).getAllByRole("tab");
    expect(tabs.map((t) => `${t.textContent}:${t.getAttribute("aria-selected")}`)).toEqual([
      "General:true",
      "Appearance:false",
      "Advanced:false",
    ]);
    expect(screen.getByRole("tabpanel").textContent).toContain("rows of general");
    expect(screen.getByText("Preferences")).toBeTruthy();
  });

  it("moves between pages with the arrow keys and on click", async () => {
    const onSelect = vi.fn();
    const user = userEvent.setup();
    render(h(Harness, { onSelect }));
    await user.click(screen.getByRole("tab", { name: "General" }));
    await user.keyboard("{ArrowDown}");
    expect(onSelect).toHaveBeenLastCalledWith("appearance");
    expect(screen.getByRole("tabpanel").textContent).toContain("rows of appearance");
    await user.click(screen.getByRole("tab", { name: "Advanced" }));
    expect(onSelect).toHaveBeenLastCalledWith("advanced");
  });

  it("shows results in a named region with no page selected while searching", async () => {
    const user = userEvent.setup();
    render(h(Harness));
    await user.type(screen.getByRole("searchbox", { name: "Search settings" }), "a");
    expect(screen.queryByRole("tabpanel")).toBeNull();
    const results = screen.getByRole("region", { name: "Search results" });
    expect(within(results).getByRole("region", { name: "General" })).toBeTruthy();
    for (const tab of screen.getAllByRole("tab")) {
      expect(tab.getAttribute("aria-selected")).toBe("false");
    }
    // Esc clears the text and the page that was open comes back.
    await user.keyboard("{Escape}");
    expect(screen.getByRole("tab", { name: "General" }).getAttribute("aria-selected")).toBe("true");
  });
});

describe("SettingRow", () => {
  function Row(props: { modified?: boolean; onReset?: () => void; note?: string }) {
    const [on, setOn] = useState(true);
    return h(
      TooltipProvider,
      null,
      h(SettingRow, {
        id: "autosave",
        label: "Save automatically",
        description: "Saves your project while you edit.",
        htmlFor: "autosave-switch",
        ...props,
        children: h(Switch, { id: "autosave-switch", checked: on, onCheckedChange: setOn }),
      }),
    );
  }

  it("names its control by the label and describes it", () => {
    render(h(Row));
    expect(screen.getByRole("switch", { name: "Save automatically" })).toBeTruthy();
    expect(screen.getByText("Saves your project while you edit.")).toBeTruthy();
  });

  it("shows the reset button only while the value differs from its default", async () => {
    const onReset = vi.fn();
    const user = userEvent.setup();
    const { rerender } = render(h(Row, { onReset }));
    expect(screen.queryByRole("button", { name: "Reset Save automatically" })).toBeNull();
    rerender(h(Row, { modified: true, onReset }));
    await user.click(screen.getByRole("button", { name: "Reset Save automatically" }));
    expect(onReset).toHaveBeenCalledTimes(1);
  });

  it("says why it cannot be changed", () => {
    render(h(Row, { note: "Needs the engine" }));
    expect(screen.getByText("Needs the engine")).toBeTruthy();
  });
});
