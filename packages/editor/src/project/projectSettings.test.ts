// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// File > Project settings… (WP-2H.08): the schema-driven form over the engine's
// `ProjectSettings`. Each change is one `project.set_settings`; the form shows the document, so
// a change made elsewhere (an agent, an undo) shows at once; "Use as defaults for new projects"
// writes the matching app preferences; and a key added to the schema gets a control with no UI
// change.
import { defaultPreferences, type PreferenceMap } from "@cutweave/preferences";
import type { ProjectSettings, RpcClient } from "@cutweave/protocol";
import { RpcError } from "@cutweave/protocol";
import { FLICKS_PER_SECOND } from "@cutweave/timeline";
import { TooltipProvider } from "@cutweave/ui";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore, initialEngineState } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { type CatalogCommand, commandCatalog } from "../palette/catalog.ts";
import { preferences } from "../prefs/prefsStore.ts";
import type { PrefsBridge } from "../session/desktop.ts";
import { historyLabel } from "../session/historyLabels.ts";
import { docStore } from "../state/docStore.ts";
import { translator } from "../timeline/errors.ts";
import { ProjectSettingsDialog } from "./ProjectSettingsDialog.tsx";
import {
  defaultsOf,
  fieldOfPointer,
  openProjectSettings,
  patchFor,
  projectSettingsStore,
  SET_SETTINGS,
  settingsForm,
  settingsSchema,
} from "./projectSettings.ts";

beforeAll(() => {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;
  Element.prototype.scrollIntoView ??= () => {};
});

const t = translator((key: string, vars?: object) => i18n.t(key as never, vars as never));

const settingsOf = (more: Partial<ProjectSettings> = {}): ProjectSettings =>
  ({
    default_format: {
      width: 1920,
      height: 1080,
      frame_rate: { num: 30, den: 1 },
      sample_rate: 48000,
      color_space: "rec709",
      background: "#000000",
    },
    ...more,
  }) as ProjectSettings;

function showProject(settings: ProjectSettings): void {
  docStore.setState({
    status: "ready",
    rev: 1,
    committedRev: 1,
    project: { settings } as never,
  });
}

function fakeEngine() {
  const call = vi.fn(async (_name: string, _args: unknown) => ({}));
  engineStore.setState({
    ...initialEngineState,
    rpc: { call } as unknown as RpcClient,
    connection: "ready",
  } as never);
  return call;
}

function fakePrefs() {
  const values: PreferenceMap = defaultPreferences();
  const calls: unknown[][] = [];
  const bridge: PrefsBridge = {
    get: async () => ({ values: { ...values }, defaults: defaultPreferences() }),
    set: async (key: string, value: unknown) => {
      calls.push([key, value]);
    },
    reset: async () => undefined,
    exportToFile: vi.fn(),
    importFromFile: vi.fn(),
    onChanged: () => () => undefined,
  } as unknown as PrefsBridge;
  return { bridge, calls };
}

function mount() {
  return render(h(I18nextProvider, { i18n }, h(TooltipProvider, null, h(ProjectSettingsDialog))));
}

let original: readonly CatalogCommand[];
beforeEach(() => {
  original = commandCatalog.getState().commands;
});
afterEach(() => {
  cleanup();
  projectSettingsStore.getState().close();
  commandCatalog.setState({ commands: original });
  docStore.setState({ status: "empty", rev: 0, committedRev: 0, project: null });
  engineStore.setState(initialEngineState);
  preferences.detach();
});

describe("the project settings form", () => {
  it("draws every key of the engine's schema", () => {
    const schema = settingsSchema();
    expect(Object.keys(schema.properties ?? {}).sort()).toEqual([
      "default_format",
      "proxy_policy",
      "still_duration",
    ]);
    const form = settingsForm(schema, settingsOf(), t);
    // Seconds for people, flicks for the engine; a project that stores none shows 5 s.
    expect(form.value.still_duration).toBe(5);
    expect(form.value.proxy_policy).toBe("auto");
    expect((form.value.default_format as { frame_rate: string }).frame_rate).toBe("30/1");
    expect(form.hints.still_duration?.group).toBe("Media");
    expect(form.hints.default_format?.group).toBe("New sequences");
  });

  it("turns an edited field into the patch of that key only", () => {
    expect(patchFor("still_duration", 3)).toEqual({ still_duration: 3 * FLICKS_PER_SECOND });
    expect(patchFor("still_duration", 0)).toBeNull();
    expect(patchFor("proxy_policy", "off")).toEqual({ proxy_policy: "off" });
    expect(patchFor("default_format.width", 1080.4)).toEqual({ default_format: { width: 1080 } });
    expect(patchFor("default_format.frame_rate", "30000/1001")).toEqual({
      default_format: { frame_rate: { num: 30000, den: 1001 } },
    });
    // A key this version does not know goes as the form gave it.
    expect(patchFor("render_cache.enabled", false)).toEqual({ render_cache: { enabled: false } });
    expect(fieldOfPointer("/patch/default_format/width")).toBe("default_format.width");
    expect(fieldOfPointer("")).toBeNull();
  });

  it("names its undo step", () => {
    expect(historyLabel(SET_SETTINGS, t)).toBe("Change project settings");
  });

  it("maps the project's values to the app preferences that are defaults", () => {
    expect(defaultsOf(settingsOf())).toEqual({ stillSeconds: 5, proxyPolicy: "auto" });
    expect(
      defaultsOf(settingsOf({ still_duration: 3 * FLICKS_PER_SECOND, proxy_policy: "all" })),
    ).toEqual({ stillSeconds: 3, proxyPolicy: "all" });
    // The preference counts whole seconds, at least one.
    expect(defaultsOf(settingsOf({ still_duration: 0.2 * FLICKS_PER_SECOND })).stillSeconds).toBe(
      1,
    );
  });
});

describe("the Project settings dialog", () => {
  it("sends one project.set_settings per change", async () => {
    const call = fakeEngine();
    showProject(settingsOf());
    mount();
    act(() => openProjectSettings());
    const dialog = await screen.findByRole("dialog", { name: "Project settings" });
    const user = userEvent.setup();
    await user.click(within(dialog).getByRole("radio", { name: "All video" }));
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith(SET_SETTINGS, { patch: { proxy_policy: "all" } }),
    );
    expect(call).toHaveBeenCalledTimes(1);
  });

  it("shows a change made elsewhere at once", async () => {
    fakeEngine();
    showProject(settingsOf());
    mount();
    act(() => openProjectSettings());
    const dialog = await screen.findByRole("dialog", { name: "Project settings" });
    const proxies = () => within(dialog).getByRole("radio", { name: "Off" });
    expect(proxies().getAttribute("aria-checked")).toBe("false");
    // An agent's `project.set_settings` arrives as a document patch.
    act(() => showProject(settingsOf({ proxy_policy: "off" })));
    expect(proxies().getAttribute("aria-checked")).toBe("true");
  });

  it("shows the engine's reason under the field it refused", async () => {
    const call = fakeEngine();
    call.mockRejectedValueOnce(
      new RpcError("invalid", "width must be between 1 and 16384", "/patch/default_format"),
    );
    showProject(settingsOf());
    mount();
    act(() => openProjectSettings());
    const dialog = await screen.findByRole("dialog", { name: "Project settings" });
    const user = userEvent.setup();
    await user.click(within(dialog).getByRole("radio", { name: "Off" }));
    expect((await within(dialog).findByRole("alert")).textContent).toContain("width must be");
  });

  it("writes the matching app preferences with Use as defaults for new projects", async () => {
    fakeEngine();
    const prefs = fakePrefs();
    await preferences.attach(prefs.bridge);
    showProject(settingsOf({ still_duration: 3 * FLICKS_PER_SECOND, proxy_policy: "off" }));
    mount();
    act(() => openProjectSettings());
    const dialog = await screen.findByRole("dialog", { name: "Project settings" });
    const user = userEvent.setup();
    await user.click(
      within(dialog).getByRole("button", { name: "Use as defaults for new projects" }),
    );
    await waitFor(() =>
      expect(prefs.calls).toEqual([
        ["editing.still_duration", 3],
        ["media.proxy_policy", "off"],
      ]),
    );
    expect((await within(dialog).findByRole("status")).textContent).toContain("3 s");
  });

  // WP-2H.08's acceptance: an optional key added to `ProjectSettings` (a test build of the
  // schema) gets a control with no change to the dialog.
  it("adds a control for a key the schema gains", async () => {
    const call = fakeEngine();
    const commands = original.map((c) => {
      if (c.name !== SET_SETTINGS) return c;
      const patch = c.inputSchema.properties?.patch;
      return {
        ...c,
        inputSchema: {
          ...c.inputSchema,
          properties: {
            ...c.inputSchema.properties,
            patch: {
              ...patch,
              properties: {
                ...patch?.properties,
                loop_preview: { type: ["boolean", "null"], description: "A key of a test build." },
              },
            },
          },
        },
      } as CatalogCommand;
    });
    commandCatalog.setState({ commands });
    showProject(settingsOf());
    mount();
    act(() => openProjectSettings());
    const dialog = await screen.findByRole("dialog", { name: "Project settings" });
    const added = within(dialog).getByRole("switch", { name: "Loop preview" });
    const user = userEvent.setup();
    await user.click(added);
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith(SET_SETTINGS, { patch: { loop_preview: true } }),
    );
  });
});
