// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h, useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { IconButton } from "./IconButton.tsx";
import { Panel, PanelDivider } from "./Panel.tsx";
import { TabList, TabPanel, Tabs } from "./Tabs.tsx";

afterEach(cleanup);

describe("Panel", () => {
  it("is a region named by its title heading", () => {
    render(h(Panel, { title: "Inspector" }, "body"));
    const region = screen.getByRole("region", { name: "Inspector" });
    expect(region.tagName).toBe("SECTION");
    expect(screen.getByRole("heading", { name: "Inspector" })).toBeTruthy();
    expect(region.textContent).toContain("body");
  });

  it("renders header actions in tab order after the title", async () => {
    const onClose = vi.fn();
    const user = userEvent.setup();
    render(
      h(
        Panel,
        {
          title: "Media",
          actions: [
            h(IconButton, { key: "a", size: "sm", icon: "search", label: "Search media" }),
            h(IconButton, {
              key: "b",
              size: "sm",
              icon: "x",
              label: "Close panel",
              onClick: onClose,
            }),
          ],
        },
        "body",
      ),
    );
    await user.tab();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Search media" }));
    await user.tab();
    await user.keyboard("{Enter}");
    expect(onClose).toHaveBeenCalled();
  });

  it("collapsed leaves the header only", () => {
    render(h(Panel, { title: "History", collapsed: true }, "hidden body"));
    expect(screen.getByRole("heading", { name: "History" })).toBeTruthy();
    expect(screen.queryByText("hidden body")).toBeNull();
  });

  it("hosts header tabs that switch bodies", async () => {
    function Harness() {
      const [tab, setTab] = useState("media");
      return h(Tabs, {
        value: tab,
        onValueChange: setTab,
        children: h(Panel, {
          title: "Assets",
          tabs: h(TabList, {
            "aria-label": "Asset browser",
            tabs: [
              { id: "media", label: "Media" },
              { id: "audio", label: "Audio" },
            ],
          }),
          children: [
            h(TabPanel, { key: "m", value: "media", children: "Media bin" }),
            h(TabPanel, { key: "a", value: "audio", children: "Audio bin" }),
          ],
        }),
      });
    }
    const user = userEvent.setup();
    render(h(Harness));
    const header = screen.getByRole("heading", { name: "Assets" }).parentElement;
    expect(header?.contains(screen.getByRole("tablist"))).toBe(true);
    await user.click(screen.getByRole("tab", { name: "Audio" }));
    expect(screen.getByRole("tabpanel").textContent).toBe("Audio bin");
  });

  it("PanelDivider is a separator", () => {
    render(h(Panel, { title: "P" }, h(PanelDivider)));
    expect(screen.getByRole("separator")).toBeTruthy();
  });
});
