// SPDX-License-Identifier: GPL-3.0-or-later
import { defaultPreferences, PREFERENCE_KEYS, type PreferenceChanges } from "@cutweave/preferences";
import { describe, expect, it, vi } from "vitest";
import type { PrefsBridge } from "../session/desktop.ts";
import { migrateLegacyView } from "./legacy.ts";
import { createPrefsStore, PreferencesClient } from "./prefsStore.ts";

function bridgeWith(values = defaultPreferences()) {
  const listeners = new Set<(changes: PreferenceChanges) => void>();
  const defaults = defaultPreferences();
  const bridge = {
    get: vi.fn(async () => ({ values: { ...values }, defaults })),
    set: vi.fn(async (key: string, value: unknown) => {
      (values as Record<string, unknown>)[key] = value;
    }),
    reset: vi.fn(async () => undefined),
    exportToFile: vi.fn(),
    importFromFile: vi.fn(),
    onChanged: (listener: (changes: PreferenceChanges) => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  } satisfies PrefsBridge;
  return { bridge, listeners, values };
}

describe("PreferencesClient", () => {
  it("without a desktop shell holds the defaults and keeps changes for the session", async () => {
    const client = new PreferencesClient(createPrefsStore());
    await client.attach(null);
    expect(client.persistent).toBe(false);
    expect(client.store.getState().loaded).toBe(true);
    await client.set("appearance.theme", "light");
    expect(client.get("appearance.theme")).toBe("light");
    await client.reset(["appearance.theme"]);
    expect(client.get("appearance.theme")).toBe("dark");
  });

  it("takes main's snapshot before it resolves, so the first paint is right", async () => {
    const { bridge } = bridgeWith({ ...defaultPreferences(), "appearance.theme": "light" });
    const client = new PreferencesClient(createPrefsStore());
    await client.attach(bridge);
    expect(client.get("appearance.theme")).toBe("light");
    expect(client.persistent).toBe(true);
  });

  it("starts with the defaults when main cannot answer", async () => {
    const { bridge } = bridgeWith();
    bridge.get.mockRejectedValueOnce(new Error("gone"));
    const client = new PreferencesClient(createPrefsStore());
    await client.attach(bridge);
    expect(client.store.getState().loaded).toBe(true);
    expect(client.get("appearance.theme")).toBe("dark");
  });

  it("writes one key at a time, at once on the store, and follows changes main announces", async () => {
    const { bridge, listeners } = bridgeWith();
    const client = new PreferencesClient(createPrefsStore());
    await client.attach(bridge);
    const pending = client.set("saving.interval", 120);
    expect(client.get("saving.interval")).toBe(120);
    await pending;
    expect(bridge.set).toHaveBeenCalledWith("saving.interval", 120);
    for (const listener of listeners) listener({ "appearance.theme": "system" });
    expect(client.get("appearance.theme")).toBe("system");
  });

  it("refuses a value the schema refuses without asking main", async () => {
    const { bridge } = bridgeWith();
    const client = new PreferencesClient(createPrefsStore());
    await client.attach(bridge);
    await expect(client.set("saving.interval", 45 as never)).rejects.toThrow(/invalid preference/);
    expect(bridge.set).not.toHaveBeenCalled();
    expect(client.get("saving.interval")).toBe(30);
  });

  it("puts the value back from main's snapshot when main refuses a write", async () => {
    const { bridge } = bridgeWith();
    const client = new PreferencesClient(createPrefsStore());
    await client.attach(bridge);
    bridge.set.mockRejectedValueOnce(new Error("refused"));
    await expect(client.set("appearance.theme", "light")).rejects.toThrow("refused");
    expect(client.get("appearance.theme")).toBe("dark");
  });

  it("resets keys to the machine's defaults and every key for reset all", async () => {
    const { bridge } = bridgeWith({ ...defaultPreferences(), "saving.backups": 20 });
    const client = new PreferencesClient(createPrefsStore());
    await client.attach(bridge);
    await client.reset(["saving.backups"]);
    expect(bridge.reset).toHaveBeenLastCalledWith(["saving.backups"]);
    expect(client.get("saving.backups")).toBe(10);
    await client.set("appearance.theme", "light");
    await client.resetAll();
    expect(client.store.getState().values).toEqual(client.store.getState().defaults);
    expect(bridge.reset).toHaveBeenLastCalledWith(PREFERENCE_KEYS);
  });
});

describe("migrating the old view blob (WP-1M.11)", () => {
  const blob = (state: object) => JSON.stringify({ state, version: 1 });
  const memory = (initial: Record<string, string> = {}) => {
    const data = new Map(Object.entries(initial));
    return {
      data,
      getItem: (key: string) => data.get(key) ?? null,
      setItem: (key: string, value: string) => void data.set(key, value),
    };
  };

  async function attached(values = defaultPreferences()) {
    const fake = bridgeWith(values);
    const client = new PreferencesClient(createPrefsStore());
    await client.attach(fake.bridge);
    return { ...fake, client };
  }

  it("keeps a profile's old theme and saving choices, and removes them from the blob once", async () => {
    const storage = memory({
      "cutweave.view": blob({
        theme: "light",
        autosave: { enabled: false, intervalS: 120, keepBackups: 20 },
        workspaceId: "long-form",
      }),
    });
    const { client, bridge } = await attached();
    const moved = await migrateLegacyView(storage, client);
    expect(moved.sort()).toEqual([
      "appearance.theme",
      "saving.autosave",
      "saving.backups",
      "saving.interval",
    ]);
    expect(client.get("appearance.theme")).toBe("light");
    expect(client.get("saving.autosave")).toBe(false);
    expect(bridge.set).toHaveBeenCalledWith("appearance.theme", "light");
    const left = JSON.parse(storage.data.get("cutweave.view") ?? "{}") as {
      state: Record<string, unknown>;
    };
    expect(left.state).toEqual({ workspaceId: "long-form" });
    // The second start has nothing to move.
    expect(await migrateLegacyView(storage, client)).toEqual([]);
  });

  it("does not overwrite a value the person already changed in the new store", async () => {
    const storage = memory({ "cutweave.view": blob({ theme: "light" }) });
    const { client } = await attached({ ...defaultPreferences(), "appearance.theme": "system" });
    expect(await migrateLegacyView(storage, client)).toEqual([]);
    expect(client.get("appearance.theme")).toBe("system");
    // The blob is cleaned all the same.
    expect(storage.data.get("cutweave.view")).not.toContain("theme");
  });

  it("keeps the blob for the next start when main refuses a value", async () => {
    const storage = memory({ "cutweave.view": blob({ theme: "light" }) });
    const { client, bridge } = await attached();
    bridge.set.mockRejectedValueOnce(new Error("refused"));
    expect(await migrateLegacyView(storage, client)).toEqual([]);
    expect(storage.data.get("cutweave.view")).toContain('"theme":"light"');
  });

  it("ignores values that are not choices, and does nothing without a desktop shell", async () => {
    const storage = memory({
      "cutweave.view": blob({ theme: "neon", autosave: { enabled: "yes", intervalS: 7 } }),
    });
    const { client } = await attached();
    expect(await migrateLegacyView(storage, client)).toEqual([]);
    const local = new PreferencesClient(createPrefsStore());
    await local.attach(null);
    expect(
      await migrateLegacyView(memory({ "cutweave.view": blob({ theme: "light" }) }), local),
    ).toEqual([]);
    expect(await migrateLegacyView(null, client)).toEqual([]);
    expect(await migrateLegacyView(memory({ "cutweave.view": "{broken" }), client)).toEqual([]);
  });
});
