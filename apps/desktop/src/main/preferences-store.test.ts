// SPDX-License-Identifier: GPL-3.0-or-later
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { nodeFiles, PreferenceError, PreferencesStore } from "./preferences-store.ts";

let dir: string;
let file: string;
let logs: string[];
const open = (options: ConstructorParameters<typeof PreferencesStore>[1] = {}) =>
  new PreferencesStore(file, { log: (m) => logs.push(m), ...options });
const onDisk = () => JSON.parse(readFileSync(file, "utf8")) as Record<string, unknown>;

beforeEach(() => {
  dir = mkdtempSync(path.join(tmpdir(), "cutweave-prefs-"));
  file = path.join(dir, "preferences.json");
  logs = [];
});
afterEach(() => {
  rmSync(dir, { recursive: true, force: true });
});

describe("PreferencesStore startup", () => {
  it("starts with the defaults when there is no file, and writes nothing", () => {
    const store = open();
    expect(store.get("appearance.theme")).toBe("dark");
    expect(readdirSync(dir)).toEqual([]);
  });

  it("starts with defaults and a warning for a damaged file, then rewrites it clean", () => {
    writeFileSync(file, "{ this is not json");
    const store = open();
    expect(store.get("appearance.theme")).toBe("dark");
    expect(logs.join("\n")).toContain("damaged");
    expect(onDisk()).toMatchObject({ version: 1, app: {} });
  });

  it("falls back per key for an unknown key, a wrong type and an out-of-range value", () => {
    writeFileSync(
      file,
      JSON.stringify({
        version: 1,
        app: {
          "appearance.theme": "light",
          "made.up": true,
          "saving.autosave": 1,
          "general.recent_count": 999,
        },
      }),
    );
    const store = open();
    expect(store.get("appearance.theme")).toBe("light");
    expect(store.get("saving.autosave")).toBe(true);
    expect(store.get("general.recent_count")).toBe(10);
    expect(logs).toHaveLength(3);
    // The file is clean again: only the valid, non-default key remains.
    expect(onDisk()).toEqual({ version: 1, app: { "appearance.theme": "light" }, machine: {} });
  });

  it("migrates a v0 fixture and rewrites it as v1", () => {
    writeFileSync(file, JSON.stringify({ "appearance.theme": "light", "saving.interval": 120 }));
    const store = open();
    expect(store.get("appearance.theme")).toBe("light");
    expect(store.get("saving.interval")).toBe(120);
    expect(onDisk()).toMatchObject({ version: 1, app: { "appearance.theme": "light" } });
  });

  it("uses the machine's default folder until the person chooses one", () => {
    const ctx = { "general.new_project_folder": path.join(dir, "Documents", "Cutweave") };
    expect(open({ contextDefaults: ctx }).get("general.new_project_folder")).toBe(
      ctx["general.new_project_folder"],
    );
    const store = open({ contextDefaults: ctx });
    store.set("general.new_project_folder", path.join(dir, "Mine"));
    store.reset(["general.new_project_folder"]);
    expect(store.get("general.new_project_folder")).toBe(ctx["general.new_project_folder"]);
  });

  it("removes a temp file an interrupted write left behind", () => {
    writeFileSync(`${file}.1234.tmp`, "half");
    open();
    expect(readdirSync(dir)).toEqual([]);
  });
});

describe("PreferencesStore writes", () => {
  it("validates, stores, persists and announces one key", () => {
    const store = open();
    const seen: unknown[] = [];
    store.onChange((changes) => seen.push(changes));
    store.set("appearance.theme", "system");
    expect(store.get("appearance.theme")).toBe("system");
    expect(seen).toEqual([{ "appearance.theme": "system" }]);
    expect(onDisk()).toEqual({ version: 1, app: { "appearance.theme": "system" }, machine: {} });
    // The same value again is not a change.
    store.set("appearance.theme", "system");
    expect(seen).toHaveLength(1);
  });

  it("rejects an unknown key and a bad value, and changes nothing", () => {
    const store = open();
    expect(() => store.set("nope", 1)).toThrow(PreferenceError);
    expect(() => store.set("appearance.theme", "purple")).toThrow(PreferenceError);
    expect(() => store.set("saving.interval", 45)).toThrow(PreferenceError);
    expect(store.snapshot()).toEqual(store.defaults());
  });

  it("hands out copies: changing a snapshot does not change the store", () => {
    const store = open();
    store.set("general.dialog_choices", { "close.unsaved": "discard" });
    const snap = store.snapshot();
    snap["general.dialog_choices"]["x"] = "y";
    expect(store.get("general.dialog_choices")).toEqual({ "close.unsaved": "discard" });
  });

  it("resets keys to their defaults", () => {
    const store = open();
    store.set("saving.autosave", false);
    store.set("saving.backups", 20);
    store.reset(["saving.autosave", "saving.backups"]);
    expect(store.snapshot()).toEqual(store.defaults());
    expect(onDisk()).toEqual({ version: 1, app: {}, machine: {} });
  });

  it("keeps the old file when the write is cut between the temp file and the rename", () => {
    const first = open();
    first.set("appearance.theme", "light");
    const before = readFileSync(file, "utf8");
    const files = nodeFiles({
      renameSync: () => {
        throw new Error("killed before the rename");
      },
    });
    const second = open({ files });
    second.set("appearance.theme", "dark");
    expect(readFileSync(file, "utf8")).toBe(before);
    expect(readdirSync(dir)).toEqual(["preferences.json"]);
    expect(logs.join("\n")).toContain("could not write");
    // The value still applies for this run.
    expect(second.get("appearance.theme")).toBe("dark");
  });
});

describe("PreferencesStore export and import", () => {
  it("exports app values that differ from their defaults, and nothing else", () => {
    const store = open();
    store.set("appearance.theme", "light");
    store.set("general.startup", "last");
    const { text, count } = store.exportText();
    expect(count).toBe(2);
    expect(JSON.parse(text)).toEqual({
      version: 1,
      app: { "appearance.theme": "light", "general.startup": "last" },
    });
    expect(text).not.toMatch(/recent|window|machine/i);
  });

  it("imports the valid keys and reports the skipped ones", () => {
    const store = open();
    const outcome = store.importText(
      JSON.stringify({
        version: 1,
        app: { "appearance.theme": "light", "saving.interval": 7, ghost: 1 },
      }),
    );
    expect(outcome).toMatchObject({ status: "imported", applied: ["appearance.theme"] });
    if (outcome.status === "imported") {
      expect(outcome.skipped.map((s) => s.key).sort()).toEqual(["ghost", "saving.interval"]);
    }
    expect(store.get("appearance.theme")).toBe("light");
    expect(store.get("saving.interval")).toBe(30);
  });

  it("refuses a file that is not settings", () => {
    expect(open().importText("[]")).toEqual({ status: "invalid", reason: "not-settings" });
  });
});
