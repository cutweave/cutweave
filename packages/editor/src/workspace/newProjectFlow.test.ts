// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// Settings > General > "Workspace for new projects" (WP-2H.01): File > New project opens in the
// chosen workspace, through the real ProjectController over the fake engine.
import { defaultPreferences } from "@cutweave/preferences";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { prefsStore } from "../prefs/prefsStore.ts";
import { EMPTY_STACKS, initialSessionState, sessionStore } from "../session/sessionStore.ts";
import { sessionUiStore } from "../session/sessionUiStore.ts";
import { projectController, resetProjectController } from "../session/useProject.ts";
import { docStore } from "../state/docStore.ts";
import { initialViewState, viewStore } from "../state/viewStore.ts";
import { createFakeEngine, type FakeEngine } from "../testing/fakeEngine.ts";

let engine: FakeEngine;

const status = (extra: object = {}) => ({
  state: "open",
  path: null,
  name: "Film",
  rev: 1,
  saved_rev: 1,
  dirty: false,
  read_only: null,
  recovery: null,
  ...extra,
});

beforeEach(() => {
  engine = createFakeEngine();
  engine.install();
  sessionStore.setState({
    ...initialSessionState,
    known: true,
    name: "Film",
    stacks: EMPTY_STACKS,
  });
  sessionUiStore.setState({ unsaved: null, naming: null, recent: [], busy: false });
  docStore.setState({ status: "empty", rev: 0, committedRev: 0, project: null });
  viewStore.setState({ ...initialViewState, layouts: {} });
  resetProjectController();
  engine.answer("project.close", () => ({ project: status() }));
  engine.answer("project.new", () => ({ project: status() }));
  engine.answer("project.save_as", () => ({ path: "C:\\p\\Film\\Film.cutweave" }));
});

afterEach(() => {
  engine.uninstall();
  resetProjectController();
  prefsStore.setState({ values: defaultPreferences() });
});

async function newProject(): Promise<boolean> {
  const controller = projectController();
  const flow = controller.newProject();
  for (let i = 0; i < 100 && sessionUiStore.getState().naming === null; i++) {
    await new Promise((r) => setTimeout(r, 10));
  }
  sessionUiStore.getState().naming?.resolve(controller.target("Film", "C:\\p"));
  return flow;
}

describe("a new project opens in the workspace chosen in Settings", () => {
  it("switches to Long-form when that is the choice", async () => {
    prefsStore.setState({ values: { ...defaultPreferences(), "general.workspace": "long-form" } });
    viewStore.getState().switchWorkspace("short-form");
    expect(await newProject()).toBe(true);
    expect(viewStore.getState().workspaceId).toBe("long-form");
  });

  it("switches to Short-form when that is the choice", async () => {
    prefsStore.setState({ values: { ...defaultPreferences(), "general.workspace": "short-form" } });
    viewStore.getState().switchWorkspace("long-form");
    expect(await newProject()).toBe(true);
    expect(viewStore.getState().workspaceId).toBe("short-form");
  });

  it("stays in the workspace in use when the choice is Last used", async () => {
    viewStore.getState().switchWorkspace("long-form");
    expect(await newProject()).toBe(true);
    expect(viewStore.getState().workspaceId).toBe("long-form");
  });
});

describe("a new project takes the still image duration from Settings (WP-2F.05b)", () => {
  it("passes Settings > Editing > Still image duration to project.new", async () => {
    const sent: unknown[] = [];
    engine.answer("project.new", (params: unknown) => {
      sent.push(params);
      return { project: status() };
    });
    expect(await newProject()).toBe(true);
    prefsStore.setState({ values: { ...defaultPreferences(), "editing.still_duration": 12 } });
    expect(await newProject()).toBe(true);
    expect(sent).toEqual([
      { name: "Film", still_duration: "5s", proxy_policy: "auto" },
      { name: "Film", still_duration: "12s", proxy_policy: "auto" },
    ]);
  });

  it("passes Settings > Media and import > Proxies for new projects to project.new (WP-2F.03b)", async () => {
    const sent: { proxy_policy?: string }[] = [];
    engine.answer("project.new", (params: unknown) => {
      sent.push(params as { proxy_policy?: string });
      return { project: status() };
    });
    prefsStore.setState({ values: { ...defaultPreferences(), "media.proxy_policy": "off" } });
    expect(await newProject()).toBe(true);
    prefsStore.setState({ values: { ...defaultPreferences(), "media.proxy_policy": "all" } });
    expect(await newProject()).toBe(true);
    expect(sent.map((p) => p.proxy_policy)).toEqual(["off", "all"]);
  });

  it("passes the format saved with Use as defaults for new projects to project.new (#221)", async () => {
    const sent: { format?: unknown; settings?: unknown }[] = [];
    engine.answer("project.new", (params: unknown) => {
      sent.push(params as { settings?: unknown });
      return { project: status() };
    });
    // Not set: the engine's own format applies.
    expect(await newProject()).toBe(true);
    prefsStore.setState({
      values: {
        ...defaultPreferences(),
        "editing.sequence_format": "1080x1920 30000/1001 44100 rec709 #101010",
      },
    });
    expect(await newProject()).toBe(true);
    // A value that is not a format is as good as not set.
    prefsStore.setState({
      values: { ...defaultPreferences(), "editing.sequence_format": "99999x1 30/1 48000 x #000000" },
    });
    expect(await newProject()).toBe(true);
    expect(sent.map((p) => p.settings)).toEqual([
      undefined,
      {
        default_format: {
          width: 1080,
          height: 1920,
          frame_rate: { num: 30000, den: 1001 },
          sample_rate: 44100,
          color_space: "rec709",
          background: "#101010",
        },
      },
      undefined,
    ]);
    expect(sent.every((p) => !("format" in p))).toBe(true);
  });
});
