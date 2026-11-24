// SPDX-License-Identifier: GPL-3.0-or-later
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  defaultPreferences,
  exportPreferences,
  loadPreferences,
  PREFERENCE_KEYS,
  parseImport,
  SCHEMA_VERSION,
  serializePreferences,
  validatePreference,
} from "./schema.ts";

const CATALOG = readFileSync(
  new URL("../../../docs/reference/settings.md", import.meta.url),
  "utf8",
);

describe("the schema and the catalog", () => {
  it("has a row in docs/reference/settings.md for every key", () => {
    for (const key of PREFERENCE_KEYS) {
      expect(CATALOG, key).toContain(`| \`${key}\` |`);
    }
  });
});

describe("validatePreference", () => {
  it("accepts a value of the right type and range", () => {
    expect(validatePreference("appearance.theme", "system").ok).toBe(true);
    expect(validatePreference("saving.interval", 120).ok).toBe(true);
    expect(validatePreference("general.recent_count", 0).ok).toBe(true);
    expect(validatePreference("general.new_project_folder", "").ok).toBe(true);
    expect(validatePreference("general.new_project_folder", "C:\\Projects").ok).toBe(true);
    expect(validatePreference("general.new_project_folder", "/home/a").ok).toBe(true);
    expect(validatePreference("general.dialog_choices", { "close.unsaved": "discard" }).ok).toBe(
      true,
    );
  });

  it("rejects an unknown key, a wrong type and an out-of-range value", () => {
    expect(validatePreference("nope", 1).ok).toBe(false);
    expect(validatePreference("__proto__", 1).ok).toBe(false);
    expect(validatePreference("appearance.theme", "purple").ok).toBe(false);
    expect(validatePreference("appearance.theme", 3).ok).toBe(false);
    expect(validatePreference("saving.autosave", "yes").ok).toBe(false);
    expect(validatePreference("saving.interval", 45).ok).toBe(false);
    expect(validatePreference("general.recent_count", 26).ok).toBe(false);
    expect(validatePreference("general.recent_count", 2.5).ok).toBe(false);
    expect(validatePreference("general.new_project_folder", "relative/dir").ok).toBe(false);
    expect(validatePreference("general.new_project_folder", "/a\0b").ok).toBe(false);
    expect(validatePreference("general.dialog_choices", { "Bad Key": "x" }).ok).toBe(false);
    expect(validatePreference("general.dialog_choices", [1]).ok).toBe(false);
  });
});

describe("loadPreferences", () => {
  it("returns the defaults for a missing file", () => {
    const loaded = loadPreferences(null);
    expect(loaded.values).toEqual(defaultPreferences());
    expect(loaded.warnings).toEqual([]);
    expect(loaded.rewrite).toBe(false);
  });

  it("starts with defaults and a warning for a damaged file", () => {
    for (const text of ["{ not json", "[1,2]", "42", ""]) {
      const loaded = loadPreferences(text);
      expect(loaded.values).toEqual(defaultPreferences());
      expect(loaded.warnings.length).toBeGreaterThan(0);
      expect(loaded.rewrite).toBe(true);
    }
  });

  it("drops an unknown key, a wrong type and an out-of-range value, one default each", () => {
    const text = JSON.stringify({
      version: 1,
      app: {
        "appearance.theme": "light",
        "mystery.key": 1,
        "saving.autosave": "yes",
        "saving.interval": 45,
        "general.recent_count": 12,
      },
      machine: {},
    });
    const loaded = loadPreferences(text);
    expect(loaded.values["appearance.theme"]).toBe("light");
    expect(loaded.values["general.recent_count"]).toBe(12);
    expect(loaded.values["saving.autosave"]).toBe(true);
    expect(loaded.values["saving.interval"]).toBe(30);
    expect(loaded.warnings).toHaveLength(3);
    expect(loaded.warnings.join("\n")).toContain("mystery.key");
    expect(loaded.rewrite).toBe(true);
  });

  it("migrates a v0 file (a flat map with no version)", () => {
    const loaded = loadPreferences(
      JSON.stringify({ "appearance.theme": "light", "saving.backups": 20 }),
    );
    expect(loaded.migratedFrom).toBe(0);
    expect(loaded.values["appearance.theme"]).toBe("light");
    expect(loaded.values["saving.backups"]).toBe(20);
    expect(loaded.rewrite).toBe(true);
  });

  it("reads what it knows from a newer file and asks for a rewrite", () => {
    const loaded = loadPreferences(
      JSON.stringify({ version: SCHEMA_VERSION + 1, app: { "appearance.theme": "light" } }),
    );
    expect(loaded.values["appearance.theme"]).toBe("light");
    expect(loaded.rewrite).toBe(true);
  });

  it("applies the machine's own defaults and lets the file override them", () => {
    const ctx = { "general.new_project_folder": "C:\\Users\\a\\Documents\\Cutweave" };
    expect(loadPreferences(null, ctx).values["general.new_project_folder"]).toBe(
      "C:\\Users\\a\\Documents\\Cutweave",
    );
    const chosen = serializePreferences({
      ...defaultPreferences(ctx),
      "general.new_project_folder": "D:\\Work",
    });
    expect(loadPreferences(chosen, ctx).values["general.new_project_folder"]).toBe("D:\\Work");
  });

  it("round-trips through serializePreferences, writing only what differs from the default", () => {
    const values = { ...defaultPreferences(), "appearance.theme": "light" as const };
    const text = serializePreferences(values);
    expect(JSON.parse(text)).toEqual({
      version: 1,
      app: { "appearance.theme": "light" },
      machine: {},
    });
    expect(loadPreferences(text).values).toEqual(values);
  });
});

describe("export and import", () => {
  it("exports the changed app values with the version, nothing else", () => {
    const values = {
      ...defaultPreferences(),
      "appearance.theme": "system" as const,
      "saving.backups": 20 as const,
    };
    const { text, count } = exportPreferences(values);
    expect(count).toBe(2);
    expect(JSON.parse(text)).toEqual({
      version: 1,
      app: { "appearance.theme": "system", "saving.backups": 20 },
    });
    expect(text).not.toContain("machine");
  });

  it("imports the valid keys and lists the ones it skipped", () => {
    const parsed = parseImport(
      JSON.stringify({
        version: 1,
        app: { "appearance.theme": "light", "saving.interval": 45, ghost: true },
        machine: { "audio.output_device": "x" },
      }),
    );
    expect(parsed.ok).toBe(true);
    if (!parsed.ok) return;
    expect(parsed.applied).toEqual({ "appearance.theme": "light" });
    expect(parsed.skipped.map((s) => s.key).sort()).toEqual([
      "audio.output_device",
      "ghost",
      "saving.interval",
    ]);
  });

  it("refuses a file that is not settings, unreadable, or from a newer version", () => {
    expect(parseImport("nope")).toEqual({ ok: false, reason: "unreadable" });
    expect(parseImport("{}")).toEqual({ ok: false, reason: "not-settings" });
    expect(parseImport(JSON.stringify({ version: 99, app: {} }))).toEqual({
      ok: false,
      reason: "newer",
    });
  });
});
