// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// Settings > General > On startup (WP-1M.11): "Last project" opens the newest recent project once
// the engine is connected and has said that nothing is open; "Empty project" leaves it alone.
import { defaultPreferences } from "@cutweave/preferences";
import { cleanup, render, waitFor } from "@testing-library/react";
import { createElement as h } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore, initialEngineState } from "../engine/engineStore.ts";
import { prefsStore } from "../prefs/prefsStore.ts";
import { createFakeEngine, type FakeEngine } from "../testing/fakeEngine.ts";
import { initialSessionState, sessionStore } from "./sessionStore.ts";
import { sessionUiStore } from "./sessionUiStore.ts";
import { resetProjectController, useProjectWiring } from "./useProject.ts";

function Wiring() {
  useProjectWiring();
  return null;
}

const OPENED = {
  project: {
    state: "open",
    path: "C:\\p\\Film\\Film.cutweave",
    name: "Film",
    rev: 1,
    saved_rev: 1,
    dirty: false,
    read_only: null,
    recovery: null,
  },
  missing_media: [],
  migrated_from: null,
};

let engine: FakeEngine;
const RECENT: readonly string[] = ["C:\\p\\Film\\Film.cutweave", "C:\\p\\Old"];
const list = vi.fn(async (): Promise<readonly string[]> => RECENT);

beforeEach(() => {
  list.mockReset();
  list.mockResolvedValue(RECENT);
  (globalThis as { cutweave?: unknown }).cutweave = {
    dialog: { openProjectFolder: async () => null },
    recent: { list, add: async () => undefined, remove: async () => undefined },
  };
  engine = createFakeEngine();
  engine.answer("project.open", () => OPENED);
  engine.install();
  sessionStore.setState({ ...initialSessionState });
  sessionUiStore.setState({ recent: [] });
  resetProjectController();
});

afterEach(() => {
  cleanup();
  engine.uninstall();
  engineStore.setState({ ...initialEngineState });
  prefsStore.setState({ values: defaultPreferences() });
  resetProjectController();
  delete (globalThis as { cutweave?: unknown }).cutweave;
});

const startup = (value: "empty" | "last") =>
  prefsStore.setState({ values: { ...defaultPreferences(), "general.startup": value } });

describe("On startup", () => {
  it("opens the last project when the setting says so and nothing is open", async () => {
    startup("last");
    render(h(Wiring));
    // The engine has not said what it holds yet: nothing happens.
    expect(engine.names()).not.toContain("project.open");
    sessionStore.setState({ known: true, path: null, dirty: false });
    await waitFor(() => expect(engine.names()).toContain("project.open"));
    expect(engine.calls.find((c) => c.name === "project.open")?.input).toEqual({
      path: "C:\\p\\Film\\Film.cutweave",
    });
  });

  it("starts empty by default", async () => {
    render(h(Wiring));
    sessionStore.setState({ known: true, path: null, dirty: false });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(engine.names()).not.toContain("project.open");
  });

  it("leaves a project that is already open alone", async () => {
    startup("last");
    render(h(Wiring));
    sessionStore.setState({ known: true, path: "C:\\p\\Other\\Other.cutweave", dirty: false });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(engine.names()).not.toContain("project.open");
  });

  it("decides once per launch: a later close does not reopen it", async () => {
    startup("last");
    render(h(Wiring));
    sessionStore.setState({ known: true, path: "C:\\p\\Other\\Other.cutweave", dirty: false });
    sessionStore.setState({ path: null });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(engine.names()).not.toContain("project.open");
  });

  it("opens nothing when there are no recent projects", async () => {
    startup("last");
    list.mockResolvedValue([]);
    render(h(Wiring));
    sessionStore.setState({ known: true, path: null, dirty: false });
    await waitFor(() => expect(list).toHaveBeenCalled());
    expect(engine.names()).not.toContain("project.open");
  });
});

describe("Recent projects to list", () => {
  it("refreshes the recent list when the number changes", async () => {
    render(h(Wiring));
    await waitFor(() => expect(list).toHaveBeenCalledTimes(1));
    prefsStore.setState({
      values: { ...prefsStore.getState().values, "general.recent_count": 3 },
    });
    await waitFor(() => expect(list).toHaveBeenCalledTimes(2));
    expect(sessionUiStore.getState().recent).toEqual(["C:\\p\\Film\\Film.cutweave", "C:\\p\\Old"]);
  });
});
