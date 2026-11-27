// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-2H.01: close and reopen panels, View > Panels, the empty dock, per-workspace closed sets,
// recovery from a corrupt layout. Drives the real shell in jsdom, the way a person would.

import { defaultPreferences } from "@cutweave/preferences";
import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { LAYOUT_SAVE_DELAY_MS } from "../dock/Dock.tsx";
import { binStore, initialBinState } from "../media/binStore.ts";
import { prefsStore } from "../prefs/prefsStore.ts";
import { EditorShell } from "../shell/EditorShell.tsx";
import { docStore } from "../state/docStore.ts";
import { toastStore } from "../state/toastStore.ts";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import { listWorkspaceCommands, runWorkspaceCommand } from "./commands.ts";
import { currentDock, openPanelsStore, resetDockHost } from "./dockHost.ts";
import { readLayoutMeta } from "./layoutMeta.ts";
import { applyNewProjectWorkspace, workspaceForNewProject } from "./newProject.ts";
import { registeredPanels } from "./panelRegistry.ts";
import { forgetSuggestions, suggestShortForm } from "./suggestShortForm.ts";

beforeAll(() => {
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
  toastStore.getState().clear();
});

afterEach(() => {
  cleanup();
  resetDockHost();
  delete document.documentElement.dataset.theme;
});

const region = (name: string) => screen.queryByRole("region", { name });
const closeButton = (name: string) => screen.getByRole("button", { name: `Close ${name} panel` });

async function allPanelsShown() {
  for (const name of ["Assets", "Preview", "Inspector", "Timeline"]) {
    expect(await screen.findByRole("region", { name })).toBeTruthy();
  }
}

/** View > Panels > `name`, through the real menu bar. */
async function togglePanelInMenu(user: ReturnType<typeof userEvent.setup>, name: string) {
  await user.click(screen.getByRole("menuitem", { name: "View" }));
  await user.click(await screen.findByRole("menuitem", { name: "Panels" }));
  // jsdom does not deliver pointer selection inside a submenu; the keyboard does what a click does.
  (await screen.findByRole("menuitemcheckbox", { name })).focus();
  await user.keyboard("{Enter}");
}

async function panelMenuState(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("menuitem", { name: "View" }));
  await user.click(await screen.findByRole("menuitem", { name: "Panels" }));
  const items = await screen.findAllByRole("menuitemcheckbox");
  const state = Object.fromEntries(
    items.map((i) => [i.textContent?.trim() ?? "", i.getAttribute("aria-checked")]),
  );
  await user.keyboard("{Escape}{Escape}{Escape}");
  return state;
}

describe("the panel registry", () => {
  it("holds the built-in panels with a default dock each", () => {
    expect(registeredPanels().map((p) => p.id)).toEqual([
      "assets",
      "preview",
      "inspector",
      "timeline",
      "history",
      "exports",
    ]);
  });

  it("gives every panel its own toggle command, for keys and the palette", () => {
    const ids = listWorkspaceCommands().map((c) => c.id);
    for (const id of ["assets", "preview", "inspector", "timeline"]) {
      expect(ids).toContain(`workspace.toggle_panel.${id}`);
    }
    expect(ids).toContain("workspace.reset_layout");
    expect(ids).toContain("workspace.reset_all");
    expect(runWorkspaceCommand("workspace.nonsense")).toBe(false);
  });
});

describe("Settings > General > Workspace for new projects", () => {
  it("defaults to the workspace used last and otherwise names one", () => {
    expect(defaultPreferences()["general.workspace"]).toBe("last");
    expect(workspaceForNewProject("last")).toBeNull();
    expect(workspaceForNewProject("long-form")).toBe("long-form");
    expect(workspaceForNewProject("short-form")).toBe("short-form");
  });

  it("switches the workspace for a new project when one is chosen", () => {
    viewStore.getState().switchWorkspace("short-form");
    applyNewProjectWorkspace();
    expect(viewStore.getState().workspaceId).toBe("short-form");
    prefsStore.setState({
      values: { ...defaultPreferences(), "general.workspace": "long-form" },
    });
    applyNewProjectWorkspace();
    expect(viewStore.getState().workspaceId).toBe("long-form");
  });
});

describe("suggesting Short-form for a vertical sequence", () => {
  const text = { title: "Vertical", message: "Short-form?", action: "Switch" };
  const sequence = (width: number, height: number) =>
    ({
      meta: { name: "P" },
      media: {},
      sequences: {
        seq: {
          id: "seq",
          name: "S",
          tracks: {},
          track_order: [],
          format: { width, height, frame_rate: { num: 30, den: 1 } },
        },
      },
    }) as never;

  beforeEach(() => {
    forgetSuggestions();
    docStore.setState({ project: null });
  });
  afterEach(() => docStore.setState({ project: null }));

  it("offers it once per project, only in Long-form, and never switches by itself", () => {
    docStore.setState({ project: sequence(1080, 1920) });
    viewStore.getState().switchWorkspace("short-form");
    expect(suggestShortForm("C:/p/a.cutweave", text)).toBe(false);

    viewStore.getState().switchWorkspace("long-form");
    expect(suggestShortForm("C:/p/a.cutweave", text)).toBe(true);
    expect(toastStore.getState().notices.map((n) => n.title)).toEqual(["Vertical"]);
    expect(viewStore.getState().workspaceId).toBe("long-form");
    // Once per project; another project is asked on its own.
    expect(suggestShortForm("C:/p/a.cutweave", text)).toBe(false);
    expect(suggestShortForm("C:/p/b.cutweave", text)).toBe(true);

    // Its button is the only way it switches.
    toastStore.getState().notices.at(-1)?.actions?.[0]?.onClick();
    expect(viewStore.getState().workspaceId).toBe("short-form");
  });

  it("stays quiet for a horizontal or a missing sequence", () => {
    viewStore.getState().switchWorkspace("long-form");
    expect(suggestShortForm("C:/p/c.cutweave", text)).toBe(false);
    docStore.setState({ project: sequence(1920, 1080) });
    expect(suggestShortForm("C:/p/c.cutweave", text)).toBe(false);
    expect(toastStore.getState().notices).toEqual([]);
  });
});

describe("closing and reopening panels (WP-2H.01)", () => {
  it("closes a panel with its close button and reopens it from View > Panels", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await allPanelsShown();

    await user.click(closeButton("Inspector"));
    await waitFor(() => expect(region("Inspector")).toBeNull());
    expect(region("Preview")).toBeTruthy();
    expect(await panelMenuState(user)).toEqual({
      Assets: "true",
      Preview: "true",
      Inspector: "false",
      Timeline: "true",
      History: "false",
      Exports: "false",
    });

    await togglePanelInMenu(user, "Inspector");
    expect(await screen.findByRole("region", { name: "Inspector" })).toBeTruthy();
    expect((await panelMenuState(user)).Inspector).toBe("true");
  });

  it("reopens a panel where it was last docked (its side and its size)", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await allPanelsShown();
    await user.click(closeButton("Assets"));
    await waitFor(() => expect(region("Assets")).toBeNull());
    const meta = readLayoutMeta(viewStore.getState().layouts["short-form"]);
    expect(meta.closed).toEqual(["assets"]);
    expect(meta.lastDock.assets).toMatchObject({ ref: "preview", direction: "left" });

    await togglePanelInMenu(user, "Assets");
    await screen.findByRole("region", { name: "Assets" });
    const after = readLayoutMeta(viewStore.getState().layouts["short-form"]);
    expect(after.closed).toEqual([]);
    // Assets is left of the Preview again: it comes first in the DOM order of the groups.
    const order = screen.getAllByRole("region").map((r) => r.getAttribute("aria-label"));
    expect(order.indexOf("Assets")).toBeLessThan(order.indexOf("Preview"));
  });

  it("keeps a floated panel floating after a relaunch, and closes and reopens it", async () => {
    const user = userEvent.setup();
    const first = render(h(EditorShell));
    await allPanelsShown();
    const dock = currentDock();
    const inspector = dock?.getPanel("inspector");
    if (dock === null || inspector === undefined) throw new Error("no dock");
    dock.addFloatingGroup(inspector);
    await waitFor(
      () => expect(viewStore.getState().layouts["short-form"]?.floatingGroups?.length).toBe(1),
      { timeout: LAYOUT_SAVE_DELAY_MS * 10 },
    );
    first.unmount();
    resetDockHost();

    const persisted = JSON.parse(JSON.stringify(viewStore.getState().layouts));
    viewStore.setState({ ...initialViewState, layouts: persisted });
    render(h(EditorShell));
    await allPanelsShown();
    expect(currentDock()?.toJSON().floatingGroups?.length).toBe(1);

    // A floated panel closes and comes back like any other (at its default dock).
    await user.click(closeButton("Inspector"));
    await waitFor(() => expect(region("Inspector")).toBeNull());
    await togglePanelInMenu(user, "Inspector");
    expect(await screen.findByRole("region", { name: "Inspector" })).toBeTruthy();
  });

  it("closes a panel from its tab's context menu", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await allPanelsShown();
    await user.pointer({
      keys: "[MouseRight]",
      target: screen.getByRole("tab", { name: "Inspector" }),
    });
    await user.click(await screen.findByText("Close panel"));
    await waitFor(() => expect(region("Inspector")).toBeNull());
  });

  it("keeps closed panels per workspace", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await allPanelsShown();
    await user.click(closeButton("Assets"));
    await waitFor(() => expect(region("Assets")).toBeNull());

    await user.click(screen.getByRole("radio", { name: "Long-form" }));
    expect(await screen.findByRole("region", { name: "Assets" })).toBeTruthy();

    await user.click(screen.getByRole("radio", { name: "Short-form" }));
    await screen.findByRole("region", { name: "Timeline" });
    await waitFor(() => expect(region("Assets")).toBeNull());
    expect(region("Preview")).toBeTruthy();
  });

  it("keeps a closed panel closed after a relaunch (the saved layout is the source)", async () => {
    const user = userEvent.setup();
    const first = render(h(EditorShell));
    await allPanelsShown();
    await user.click(closeButton("Inspector"));
    await waitFor(() => expect(region("Inspector")).toBeNull());
    first.unmount();
    resetDockHost();

    // A new launch reads the persisted layouts, as the view store does from storage.
    const persisted = JSON.parse(JSON.stringify(viewStore.getState().layouts));
    viewStore.setState({ ...initialViewState, layouts: persisted });
    render(h(EditorShell));
    await screen.findByRole("region", { name: "Timeline" });
    expect(region("Inspector")).toBeNull();
    expect(openPanelsStore.getState().closed).toEqual(["inspector"]);
  });

  it("shows an empty dock with Reopen panels and Reset layout when every panel is closed", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await allPanelsShown();
    for (const name of ["Assets", "Preview", "Inspector", "Timeline"]) {
      await user.click(closeButton(name));
      await waitFor(() => expect(region(name)).toBeNull());
    }
    expect(await screen.findByText("Every panel is closed")).toBeTruthy();

    // Reset layout brings all four back.
    const main = screen.getByRole("main");
    await user.click(within(main).getByRole("button", { name: "Reset layout" }));
    await allPanelsShown();
    expect(screen.queryByText("Every panel is closed")).toBeNull();
  });

  it("reopens every closed panel from the empty dock", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await allPanelsShown();
    for (const name of ["Assets", "Preview", "Inspector", "Timeline"]) {
      await user.click(closeButton(name));
      await waitFor(() => expect(region(name)).toBeNull());
    }
    await user.click(await screen.findByRole("button", { name: "Reopen panels" }));
    await allPanelsShown();
  });

  it("View > Reset all workspaces brings every panel back in every workspace", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await allPanelsShown();
    await user.click(closeButton("Assets"));
    await waitFor(() => expect(region("Assets")).toBeNull());
    await user.click(screen.getByRole("radio", { name: "Long-form" }));
    await screen.findByRole("region", { name: "Assets" });
    await user.click(closeButton("Inspector"));
    await waitFor(() => expect(region("Inspector")).toBeNull());

    await user.click(screen.getByRole("menuitem", { name: "View" }));
    await user.click(await screen.findByRole("menuitem", { name: "Reset all workspaces" }));
    await allPanelsShown();
    await waitFor(() => expect(viewStore.getState().layouts["short-form"]).toBeUndefined());
    expect(readLayoutMeta(viewStore.getState().layouts["long-form"]).closed).toEqual([]);

    await user.click(screen.getByRole("radio", { name: "Short-form" }));
    await allPanelsShown();
  });

  it("keeps a panel's state in a store, so closing it loses nothing", async () => {
    const user = userEvent.setup();
    binStore.setState({ ...initialBinState, text: "beach" });
    viewStore.getState().setZoom(240);
    viewStore.getState().setScroll(120, 0);
    render(h(EditorShell));
    await allPanelsShown();
    await user.click(closeButton("Assets"));
    await user.click(closeButton("Timeline"));
    await waitFor(() => expect(region("Timeline")).toBeNull());
    await togglePanelInMenu(user, "Assets");
    await togglePanelInMenu(user, "Timeline");
    await allPanelsShown();
    expect(binStore.getState().text).toBe("beach");
    expect(viewStore.getState().zoom).toBe(240);
    expect(viewStore.getState().scroll.x).toBe(120);
  });

  it("falls back to the default layout and says so when the saved one is corrupt", async () => {
    viewStore.setState({
      layouts: { "short-form": { grid: "nonsense", panels: 7 } as never },
    });
    render(h(EditorShell));
    await allPanelsShown();
    await waitFor(() =>
      expect(toastStore.getState().notices.map((n) => n.title)).toContain("Layout reset"),
    );
  });

  it("falls back too when a saved layout names a panel nobody registers", async () => {
    render(h(EditorShell));
    await allPanelsShown();
    await waitFor(() => expect(viewStore.getState().layouts["short-form"]).toBeDefined(), {
      timeout: LAYOUT_SAVE_DELAY_MS * 10,
    });
    cleanup();
    resetDockHost();
    const saved = JSON.parse(JSON.stringify(viewStore.getState().layouts["short-form"]));
    saved.panels.ghost = { ...saved.panels.assets, id: "ghost" };
    viewStore.setState({ layouts: { "short-form": saved } });
    toastStore.getState().clear();
    render(h(EditorShell));
    await allPanelsShown();
    await waitFor(() =>
      expect(toastStore.getState().notices.map((n) => n.title)).toContain("Layout reset"),
    );
  });

  it("adds a panel registered after a layout was saved at its default dock", async () => {
    const user = userEvent.setup();
    render(h(EditorShell));
    await allPanelsShown();
    // Close one so the saved layout lacks it, then drop it from `closed`: it must reappear.
    await user.click(closeButton("Inspector"));
    await waitFor(() => expect(region("Inspector")).toBeNull());
    cleanup();
    resetDockHost();
    const saved = JSON.parse(JSON.stringify(viewStore.getState().layouts["short-form"]));
    delete saved.cutweave;
    viewStore.setState({ layouts: { "short-form": saved } });
    render(h(EditorShell));
    expect(await screen.findByRole("region", { name: "Inspector" })).toBeTruthy();
  });
});
