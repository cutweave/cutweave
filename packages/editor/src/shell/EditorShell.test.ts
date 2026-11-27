// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom

import { defaultPreferences } from "@cutweave/preferences";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { createElement as h } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { LAYOUT_SAVE_DELAY_MS } from "../dock/Dock.tsx";
import { preferences, prefsStore } from "../prefs/prefsStore.ts";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import { EditorShell } from "./EditorShell.tsx";

beforeAll(() => {
  // jsdom has no layout engine; dockview observes sizes and Radix measures popper content.
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;
  globalThis.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
    onchange: null,
  })) as unknown as typeof matchMedia;
  Element.prototype.scrollIntoView ??= () => {};
});

beforeEach(() => {
  viewStore.setState({ ...initialViewState, layouts: {} });
  prefsStore.setState({ values: defaultPreferences() });
});

afterEach(() => {
  cleanup();
  delete document.documentElement.dataset.theme;
  delete document.documentElement.dataset.motion;
});

describe("EditorShell", () => {
  it("renders the top bar and the four docked panels", async () => {
    render(h(EditorShell));
    expect(screen.getByRole("banner")).toBeTruthy();
    expect(screen.getByRole("main")).toBeTruthy();
    for (const name of ["Assets", "Preview", "Inspector", "Timeline"]) {
      expect(await screen.findByRole("region", { name })).toBeTruthy();
    }
    expect(screen.getByRole("button", { name: "Export" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Undo" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Redo" })).toBeTruthy();
  });

  it("shows the asset browser tabs, Media first", async () => {
    render(h(EditorShell));
    const list = await screen.findByRole("tablist", { name: "Asset browser" });
    const names = within(list)
      .getAllByRole("tab")
      .map((tab) => tab.textContent);
    expect(names).toEqual([
      "Media",
      "Audio",
      "Text",
      "Stickers",
      "Effects",
      "Transitions",
      "Filters",
      "Templates",
      "Captions",
      "AI Studio",
    ]);
    expect(screen.getByRole("tab", { name: "Media" }).getAttribute("aria-selected")).toBe("true");
    expect(screen.getByText("No media yet")).toBeTruthy();
  });

  it("switches tabs and keeps the choice in the view store", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await user.click(await screen.findByRole("tab", { name: "Captions" }));
    expect(viewStore.getState().assetTab).toBe("captions");
    expect(screen.getByText(/Generate captions from speech/)).toBeTruthy();
  });

  it("switches theme through data-theme, from Settings (Ctrl+,)", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await screen.findByRole("region", { name: "Timeline" });
    expect(document.documentElement.dataset.theme).toBe("dark");
    // The title bar has no theme switch; it is in File > Settings….
    expect(screen.queryByRole("radiogroup", { name: "Theme" })).toBeNull();
    await user.keyboard("{Control>},{/Control}");
    const settings = await screen.findByRole("dialog", { name: "Settings" });
    await user.click(within(settings).getByRole("tab", { name: "Appearance" }));
    await user.click(within(settings).getByRole("radio", { name: "Light" }));
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(prefsStore.getState().values["appearance.theme"]).toBe("light");
    await user.click(within(settings).getByRole("radio", { name: "Dark" }));
    expect(document.documentElement.dataset.theme).toBe("dark");
    await user.click(within(settings).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull());
  });

  it("switches workspace and adopts its preview aspect", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await screen.findByRole("region", { name: "Preview" });
    const shortForm = screen.getByRole("radio", { name: "Short-form" });
    const longForm = screen.getByRole("radio", { name: "Long-form" });
    expect(shortForm.getAttribute("aria-checked")).toBe("true");
    expect(screen.getByRole("radio", { name: "9:16" }).getAttribute("aria-checked")).toBe("true");

    await user.click(longForm);
    expect(viewStore.getState().workspaceId).toBe("long-form");
    expect(longForm.getAttribute("aria-checked")).toBe("true");
    expect(screen.getByRole("radio", { name: "16:9" }).getAttribute("aria-checked")).toBe("true");
    // The dock rebuilds for the new workspace and the panels are still there.
    for (const name of ["Assets", "Preview", "Inspector", "Timeline"]) {
      expect(await screen.findByRole("region", { name })).toBeTruthy();
    }
  });

  it("saves a layout per workspace and restores it on return", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await screen.findByRole("region", { name: "Timeline" });
    await waitFor(() => expect(viewStore.getState().layouts["short-form"]).toBeDefined(), {
      timeout: LAYOUT_SAVE_DELAY_MS * 10,
    });
    expect(viewStore.getState().layouts["long-form"]).toBeUndefined();

    await user.click(screen.getByRole("radio", { name: "Long-form" }));
    await waitFor(() => expect(viewStore.getState().layouts["long-form"]).toBeDefined(), {
      timeout: LAYOUT_SAVE_DELAY_MS * 10,
    });
    const shortLayout = viewStore.getState().layouts["short-form"];

    await user.click(screen.getByRole("radio", { name: "Short-form" }));
    await screen.findByRole("region", { name: "Timeline" });
    expect(viewStore.getState().layouts["short-form"]).toEqual(shortLayout);
  });

  it("resets the active workspace layout to its default", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await screen.findByRole("region", { name: "Timeline" });
    await waitFor(() => expect(viewStore.getState().layouts["short-form"]).toBeDefined(), {
      timeout: LAYOUT_SAVE_DELAY_MS * 10,
    });
    const revision = viewStore.getState().layoutRevision;
    await user.click(screen.getByRole("button", { name: "Reset layout" }));
    expect(viewStore.getState().layoutRevision).toBe(revision + 1);
    for (const name of ["Assets", "Preview", "Inspector", "Timeline"]) {
      expect(await screen.findByRole("region", { name })).toBeTruthy();
    }
  });

  it("sets the density from Settings > Appearance: Standard is compact (WP-2H.09)", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await screen.findByRole("region", { name: "Timeline" });
    const root = document.documentElement;
    expect(root.dataset.density).toBe("compact");
    await user.keyboard("{Control>},{/Control}");
    const settings = await screen.findByRole("dialog", { name: "Settings" });
    await user.click(within(settings).getByRole("tab", { name: "Appearance" }));
    const density = within(settings).getByRole("radiogroup", { name: "Density" });
    await user.click(within(density).getByRole("radio", { name: "Comfortable" }));
    expect(root.dataset.density).toBe("comfortable");
    expect(prefsStore.getState().values["appearance.density"]).toBe("comfortable");
    await user.click(within(density).getByRole("radio", { name: "Standard" }));
    expect(root.dataset.density).toBe("compact");
    await user.click(within(settings).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull());
  });

  it("sets data-motion for Reduce motion On and Off, and none for the system's (WP-2H.09)", async () => {
    render(h(EditorShell));
    await screen.findByRole("region", { name: "Timeline" });
    const root = document.documentElement;
    expect(root.hasAttribute("data-motion")).toBe(false);
    await act(async () => preferences.set("appearance.reduce_motion", "on"));
    expect(root.dataset.motion).toBe("reduce");
    await act(async () => preferences.set("appearance.reduce_motion", "off"));
    expect(root.dataset.motion).toBe("full");
    await act(async () => preferences.set("appearance.reduce_motion", "system"));
    expect(root.hasAttribute("data-motion")).toBe(false);
  });

  it("offers the five interface scales and stores the chosen one (WP-2H.09)", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await screen.findByRole("region", { name: "Timeline" });
    await user.keyboard("{Control>},{/Control}");
    const settings = await screen.findByRole("dialog", { name: "Settings" });
    await user.click(within(settings).getByRole("tab", { name: "Appearance" }));
    const scale = within(settings).getByRole("radiogroup", { name: "Interface scale" });
    expect(
      within(scale)
        .getAllByRole("radio")
        .map((radio) => radio.textContent),
    ).toEqual(["90%", "100%", "110%", "125%", "150%"]);
    await user.click(within(scale).getByRole("radio", { name: "125%" }));
    expect(prefsStore.getState().values["appearance.scale"]).toBe(125);
    await user.click(within(settings).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull());
  });

  it("has no axe violations (dark and light)", async () => {
    render(h(EditorShell));
    await screen.findByRole("region", { name: "Timeline" });
    for (const theme of ["dark", "light"] as const) {
      await act(async () => preferences.set("appearance.theme", theme));
      const results = await axe.run(document.body, {
        // jsdom has no layout or computed colours, so contrast is covered by the ui axe suite.
        rules: { "color-contrast": { enabled: false } },
      });
      expect(results.violations.map((v) => `${v.id}: ${v.nodes[0]?.html}`)).toEqual([]);
    }
  });
});
