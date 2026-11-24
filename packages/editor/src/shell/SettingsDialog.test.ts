// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// File > Settings… (WP-1M.11): the tabbed frame, its search, the rows on the preferences store,
// reset, import and export, and the deep link.
import {
  defaultPreferences,
  type PreferenceChanges,
  type PreferenceMap,
} from "@cutweave/preferences";
import { TooltipProvider } from "@cutweave/ui";
import { act, cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { engineStore, initialEngineState } from "../engine/engineStore.ts";
import { i18n } from "../i18n/index.ts";
import { preferences, prefsStore } from "../prefs/prefsStore.ts";
import type { PrefsBridge } from "../session/desktop.ts";
import { sessionUiStore } from "../session/sessionUiStore.ts";
import { openSettings, settingsStore } from "../settings/settingsStore.ts";
import { SettingsDialog } from "./SettingsDialog.tsx";

beforeAll(() => {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  globalThis.ResizeObserver ??= ResizeObserverStub as unknown as typeof ResizeObserver;
  Element.prototype.scrollIntoView ??= () => {};
});

/** A preferences bridge that behaves like main's store, and records what was asked of it. */
function fakeBridge(initial: Partial<PreferenceMap> = {}) {
  const values: PreferenceMap = { ...defaultPreferences(), ...initial };
  const defaults = defaultPreferences();
  const listeners = new Set<(changes: PreferenceChanges) => void>();
  const calls: unknown[][] = [];
  const emit = (changes: PreferenceChanges) => {
    for (const listener of listeners) listener(changes);
  };
  const bridge: PrefsBridge = {
    get: async () => ({ values: { ...values }, defaults }),
    set: async (key, value) => {
      calls.push(["set", key, value]);
      (values as Record<string, unknown>)[key] = value;
      emit({ [key]: value } as PreferenceChanges);
    },
    reset: async (keys) => {
      calls.push(["reset", ...keys]);
      const changes: Record<string, unknown> = {};
      for (const key of keys) {
        changes[key] = (defaults as Record<string, unknown>)[key];
        (values as Record<string, unknown>)[key] = changes[key];
      }
      emit(changes as PreferenceChanges);
    },
    exportToFile: vi.fn(async () => ({
      status: "saved" as const,
      path: "C:\\out\\cutweave-settings.json",
      count: 2,
    })),
    importFromFile: vi.fn(async () => ({
      status: "imported" as const,
      applied: ["appearance.theme"],
      skipped: [{ key: "ghost", reason: "unknown setting" }],
    })),
    onChanged: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
  return { bridge, calls, values };
}

let current: ReturnType<typeof fakeBridge>;

beforeEach(async () => {
  current = fakeBridge();
  await preferences.attach(current.bridge);
  engineStore.setState({ ...initialEngineState, connection: "ready" });
  settingsStore.getState().setOpen(true);
});

afterEach(async () => {
  cleanup();
  settingsStore.getState().setOpen(false);
  await preferences.attach(null);
  prefsStore.setState({ values: defaultPreferences() });
  engineStore.setState({ ...initialEngineState });
  sessionUiStore.getState().setRecent([]);
});

const render_ = () =>
  render(h(I18nextProvider, { i18n }, h(TooltipProvider, null, h(SettingsDialog))));

const dialog = () => screen.findByRole("dialog", { name: "Settings" });
const radios = (group: HTMLElement) =>
  within(group)
    .getAllByRole("radio")
    .map((r) => `${r.textContent}:${r.getAttribute("aria-checked")}`);

describe("Settings: the frame", () => {
  it("lists the pages in groups and opens on General, with the search field focused", async () => {
    render_();
    const settings = await dialog();
    const tabs = within(within(settings).getByRole("tablist", { name: "Settings pages" }))
      .getAllByRole("tab")
      .map((t) => `${t.textContent}:${t.getAttribute("aria-selected")}`);
    expect(tabs).toEqual([
      "General:true",
      "Appearance:false",
      "Saving and backups:false",
      "Advanced:false",
    ]);
    expect(within(settings).getByText("Preferences")).toBeTruthy();
    expect(within(settings).getByRole("heading", { name: "General" })).toBeTruthy();
    expect(document.activeElement).toBe(
      within(settings).getByRole("searchbox", { name: "Search settings" }),
    );
  });

  it("shows the General rows with their defaults", async () => {
    render_();
    const settings = await dialog();
    expect(radios(within(settings).getByRole("radiogroup", { name: "On startup" }))).toEqual([
      "Empty project:true",
      "Last project:false",
    ]);
    expect(
      (
        within(settings).getByRole("spinbutton", {
          name: "Recent projects to list",
        }) as HTMLInputElement
      ).value,
    ).toBe("10");
    expect(within(settings).getByRole("button", { name: "Clear recent projects" })).toBeTruthy();
    // Nothing differs from its default, so no row has a reset button, and the page has none.
    expect(within(settings).queryByRole("button", { name: /^Reset (?!choices)/ })).toBeNull();
  });

  it("changes the theme at once, on the store, and shows its reset", async () => {
    const user = userEvent.setup();
    render_();
    const settings = await dialog();
    await user.click(within(settings).getByRole("tab", { name: "Appearance" }));
    const theme = within(settings).getByRole("radiogroup", { name: "Theme" });
    expect(radios(theme)).toEqual(["Dark:true", "Light:false", "Match system:false"]);
    await user.click(within(theme).getByRole("radio", { name: "Light" }));
    expect(prefsStore.getState().values["appearance.theme"]).toBe("light");
    expect(current.calls).toContainEqual(["set", "appearance.theme", "light"]);
    await user.click(within(settings).getByRole("button", { name: "Reset Theme" }));
    expect(prefsStore.getState().values["appearance.theme"]).toBe("dark");
    expect(current.calls).toContainEqual(["reset", "appearance.theme"]);
  });

  it("moves between pages with the arrow keys", async () => {
    const user = userEvent.setup();
    render_();
    const settings = await dialog();
    await user.click(within(settings).getByRole("tab", { name: "General" }));
    await user.keyboard("{ArrowDown}");
    expect(within(settings).getByRole("heading", { name: "Appearance" })).toBeTruthy();
  });

  it("closes with Done and with Escape", async () => {
    const user = userEvent.setup();
    render_();
    const settings = await dialog();
    await user.click(within(settings).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull());
    act(() => settingsStore.getState().setOpen(true));
    await dialog();
    await user.click(screen.getByRole("tab", { name: "General" }));
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull());
  });
});

describe("Settings > Saving and backups (WP-1M.10's rows, on the store)", () => {
  async function openSaving() {
    const user = userEvent.setup();
    render_();
    const settings = await dialog();
    await user.click(within(settings).getByRole("tab", { name: "Saving and backups" }));
    return { user, settings };
  }

  it("shows the defaults: automatic, every 30 s, 10 backups", async () => {
    const { settings } = await openSaving();
    expect(
      within(settings)
        .getByRole("switch", { name: "Save automatically" })
        .getAttribute("aria-checked"),
    ).toBe("true");
    expect(
      radios(within(settings).getByRole("radiogroup", { name: "Save at least every" })),
    ).toEqual(["30 s:true", "1 min:false", "2 min:false", "5 min:false"]);
    expect(radios(within(settings).getByRole("radiogroup", { name: "Backups to keep" }))).toEqual([
      "5:false",
      "10:true",
      "20:false",
    ]);
  });

  it("changes the preference at once, and the choices follow the switch", async () => {
    const { user, settings } = await openSaving();
    await user.click(within(settings).getByRole("radio", { name: "2 min" }));
    await user.click(within(settings).getByRole("radio", { name: "20" }));
    expect(prefsStore.getState().values).toMatchObject({
      "saving.autosave": true,
      "saving.interval": 120,
      "saving.backups": 20,
    });
    await user.click(within(settings).getByRole("switch", { name: "Save automatically" }));
    expect(prefsStore.getState().values["saving.autosave"]).toBe(false);
    expect(within(settings).getByRole("radio", { name: "5 min" }).hasAttribute("disabled")).toBe(
      true,
    );
  });

  it("is disabled with a note while the engine is not connected", async () => {
    engineStore.setState({ ...initialEngineState, connection: "reconnecting" });
    const { settings } = await openSaving();
    expect(
      within(settings).getByRole("switch", { name: "Save automatically" }).hasAttribute("disabled"),
    ).toBe(true);
    expect(within(settings).getAllByText("Needs the engine")).toHaveLength(3);
  });
});

describe("Settings: search", () => {
  it("finds rows across pages, grouped by page, with the match marked", async () => {
    const user = userEvent.setup();
    render_();
    const settings = await dialog();
    await user.type(within(settings).getByRole("searchbox", { name: "Search settings" }), "auto");
    const results = within(settings).getByRole("region", { name: "Search results" });
    const group = within(results).getByRole("region", { name: "Saving and backups" });
    // Autosave, its interval and its backups all carry "autosave" as a keyword.
    // (jsdom puts a space around the <mark>; a browser's accessible name has none.)
    expect(within(group).getByRole("switch", { name: /^Save\s?auto\s?matically$/ })).toBeTruthy();
    expect(within(group).getByRole("radiogroup", { name: "Save at least every" })).toBeTruthy();
    expect(within(group).getByRole("radiogroup", { name: "Backups to keep" })).toBeTruthy();
    expect(within(group).getByText("auto", { selector: "mark" })).toBeTruthy();
    expect(within(settings).getByText("3 found")).toBeTruthy();
    expect(within(settings).getByRole("heading", { name: "Results for “auto”" })).toBeTruthy();
    // No page is selected while the results show.
    for (const tab of within(settings).getAllByRole("tab")) {
      expect(tab.getAttribute("aria-selected")).toBe("false");
    }
  });

  it("matches words in any order, ignoring case and accents", async () => {
    const user = userEvent.setup();
    render_();
    const settings = await dialog();
    await user.type(
      within(settings).getByRole("searchbox", { name: "Search settings" }),
      "PROJECTS recent",
    );
    const results = within(settings).getByRole("region", { name: "Search results" });
    expect(
      within(results).getByRole("spinbutton", { name: "Recent projects to list" }),
    ).toBeTruthy();
  });

  it("says when nothing matches, and Esc clears the search and returns to the page that was open", async () => {
    const user = userEvent.setup();
    render_();
    const settings = await dialog();
    await user.click(within(settings).getByRole("tab", { name: "Appearance" }));
    const field = within(settings).getByRole("searchbox", { name: "Search settings" });
    await user.type(field, "zzz");
    expect(within(settings).getByText("No settings match “zzz”")).toBeTruthy();
    await user.keyboard("{Escape}");
    expect((field as HTMLInputElement).value).toBe("");
    // The dialog is still open, and the Appearance page is back.
    expect(screen.queryByRole("dialog", { name: "Settings" })).not.toBeNull();
    expect(within(settings).getByRole("heading", { name: "Appearance" })).toBeTruthy();
    expect(
      within(settings).getByRole("tab", { name: "Appearance" }).getAttribute("aria-selected"),
    ).toBe("true");
  });

  it("leaves the field for the page list on a second Esc", async () => {
    const user = userEvent.setup();
    render_();
    const settings = await dialog();
    await user.keyboard("{Escape}");
    expect(document.activeElement).toBe(within(settings).getByRole("tab", { name: "General" }));
    expect(screen.queryByRole("dialog", { name: "Settings" })).not.toBeNull();
  });
});

describe("Settings: reset", () => {
  it("resets one row, then the whole page", async () => {
    const user = userEvent.setup();
    current = fakeBridge({ "general.startup": "last", "general.recent_count": 3 });
    await preferences.attach(current.bridge);
    render_();
    const settings = await dialog();
    await user.click(within(settings).getByRole("button", { name: "Reset On startup" }));
    expect(prefsStore.getState().values["general.startup"]).toBe("empty");
    expect(prefsStore.getState().values["general.recent_count"]).toBe(3);
    await user.click(within(settings).getByRole("button", { name: "Reset page" }));
    expect(prefsStore.getState().values["general.recent_count"]).toBe(10);
    expect(within(settings).queryByRole("button", { name: "Reset page" })).toBeNull();
  });

  it("asks before resetting everything, and does nothing on Cancel", async () => {
    const user = userEvent.setup();
    current = fakeBridge({ "appearance.theme": "light", "saving.backups": 20 });
    await preferences.attach(current.bridge);
    render_();
    const settings = await dialog();
    await user.click(within(settings).getByRole("tab", { name: "Advanced" }));
    await user.click(within(settings).getByRole("button", { name: "Reset all settings…" }));
    const confirm = await screen.findByRole("dialog", { name: "Reset all settings?" });
    await user.click(within(confirm).getByRole("button", { name: "Cancel" }));
    expect(prefsStore.getState().values["appearance.theme"]).toBe("light");
    await user.click(within(settings).getByRole("button", { name: "Reset all settings…" }));
    await user.click(
      within(await screen.findByRole("dialog", { name: "Reset all settings?" })).getByRole(
        "button",
        {
          name: "Reset all settings",
        },
      ),
    );
    await waitFor(() =>
      expect(prefsStore.getState().values).toEqual(prefsStore.getState().defaults),
    );
    expect(within(settings).getByRole("status").textContent).toContain("back to their defaults");
  });
});

describe("Settings > Advanced: export and import", () => {
  it("exports through main and says where the file went", async () => {
    const user = userEvent.setup();
    render_();
    const settings = await dialog();
    await user.click(within(settings).getByRole("tab", { name: "Advanced" }));
    await user.click(within(settings).getByRole("button", { name: "Export settings…" }));
    expect(current.bridge.exportToFile).toHaveBeenCalledTimes(1);
    await waitFor(() =>
      expect(within(settings).getByRole("status").textContent).toBe(
        "Settings exported to C:\\out\\cutweave-settings.json.",
      ),
    );
  });

  it("imports through main and reports what it skipped", async () => {
    const user = userEvent.setup();
    render_();
    const settings = await dialog();
    await user.click(within(settings).getByRole("tab", { name: "Advanced" }));
    await user.click(within(settings).getByRole("button", { name: "Import settings…" }));
    await waitFor(() =>
      expect(within(settings).getByRole("status").textContent).toBe(
        "Settings imported: 1. Skipped: ghost (unknown setting).",
      ),
    );
  });

  it("says when the file is not settings", async () => {
    vi.mocked(current.bridge.importFromFile).mockResolvedValueOnce({
      status: "invalid",
      reason: "not-settings",
    });
    const user = userEvent.setup();
    render_();
    const settings = await dialog();
    await user.click(within(settings).getByRole("tab", { name: "Advanced" }));
    await user.click(within(settings).getByRole("button", { name: "Import settings…" }));
    await waitFor(() =>
      expect(within(settings).getByRole("status").textContent).toBe(
        "That file is not a Cutweave settings file.",
      ),
    );
  });
});

describe("Settings: General actions", () => {
  it("clears the recent projects", async () => {
    const clear = vi.fn(async () => undefined);
    (globalThis as { cutweave?: unknown }).cutweave = {
      dialog: { openProjectFolder: async () => null },
      recent: {
        list: async () => [],
        add: async () => undefined,
        remove: async () => undefined,
        clear,
      },
    };
    try {
      sessionUiStore.getState().setRecent(["C:\\p\\a"]);
      const user = userEvent.setup();
      render_();
      const settings = await dialog();
      await user.click(within(settings).getByRole("button", { name: "Clear recent projects" }));
      await waitFor(() => expect(clear).toHaveBeenCalledTimes(1));
      expect(sessionUiStore.getState().recent).toEqual([]);
    } finally {
      delete (globalThis as { cutweave?: unknown }).cutweave;
    }
  });

  it("resets remembered 'Don't ask again' choices", async () => {
    current = fakeBridge({ "general.dialog_choices": { "close.unsaved": "discard" } });
    await preferences.attach(current.bridge);
    const user = userEvent.setup();
    render_();
    const settings = await dialog();
    expect(within(settings).getByText("1 stored")).toBeTruthy();
    await user.click(within(settings).getByRole("button", { name: "Reset choices" }));
    expect(prefsStore.getState().values["general.dialog_choices"]).toEqual({});
    expect(within(settings).getByText("None stored")).toBeTruthy();
  });
});

describe("Settings: deep link", () => {
  it("opens on the page of a row and focuses it", async () => {
    act(() => settingsStore.getState().setOpen(false));
    render_();
    act(() => openSettings("saving", "saving.interval"));
    const settings = await dialog();
    expect(within(settings).getByRole("heading", { name: "Saving and backups" })).toBeTruthy();
    await waitFor(() =>
      expect(document.activeElement?.closest('[data-setting="saving.interval"]')).not.toBeNull(),
    );
    expect(
      settings.querySelector('[data-setting="saving.interval"]')?.getAttribute("data-highlighted"),
    ).toBe("true");
  });
});

describe("Settings: accessibility", () => {
  it("has no axe violations", async () => {
    const user = userEvent.setup();
    render_();
    const settings = await dialog();
    for (const page of ["General", "Appearance", "Saving and backups", "Advanced"]) {
      await user.click(within(settings).getByRole("tab", { name: page }));
      const results = await axe.run(document.body, {
        rules: { "color-contrast": { enabled: false } },
      });
      expect(
        results.violations.map((v) => `${v.id}: ${v.nodes[0]?.html}`),
        page,
      ).toEqual([]);
    }
  });
});
