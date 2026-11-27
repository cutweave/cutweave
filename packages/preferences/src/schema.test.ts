// SPDX-License-Identifier: GPL-3.0-or-later
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  DENSITY_CHOICES,
  defaultPreferences,
  densityAttribute,
  exportPreferences,
  INTERFACE_SCALES,
  isRendererWritable,
  loadPreferences,
  MAX_AGENT_FOLDERS,
  motionAttribute,
  PREFERENCE_KEYS,
  parseImport,
  REDUCE_MOTION_CHOICES,
  SCHEMA_VERSION,
  samePreferenceValue,
  serializePreferences,
  validatePreference,
  zoomFactorFor,
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

describe("Editing and Timeline (WP-2A.13)", () => {
  it("starts at the catalog's defaults", () => {
    const d = defaultPreferences();
    expect(d["editing.link_av"]).toBe(true);
    expect(d["editing.drop_mode"]).toBe("overwrite");
    expect(d["editing.nudge"]).toBe(1);
    expect(d["editing.undo_steps"]).toBe(1000);
    expect(d["timeline.snap"]).toBe(true);
    expect(d["timeline.snap_targets"]).toEqual(["playhead", "edges", "markers", "beats"]);
    expect(d["timeline.snap_distance"]).toBe(8);
    expect(d["timeline.follow"]).toBe("page");
    expect(d["timeline.thumbnails"]).toBe("filmstrip");
    expect(d["timeline.waveforms"]).toBe(true);
    expect(d["timeline.track_height"]).toBe("medium");
    expect(d["timeline.wheel"]).toBe("scroll");
    expect(d["timeline.time_display"]).toBe("timecode");
    expect(d["timeline.drop_frame"]).toBe(true);
  });

  it("keeps the snap targets a set of the known ones, in the catalog's order", () => {
    expect(validatePreference("timeline.snap_targets", [])).toEqual({ ok: true, value: [] });
    expect(validatePreference("timeline.snap_targets", ["beats", "playhead"])).toEqual({
      ok: true,
      value: ["playhead", "beats"],
    });
    expect(validatePreference("timeline.snap_targets", ["edges", "edges"]).ok).toBe(false);
    expect(validatePreference("timeline.snap_targets", ["grid"]).ok).toBe(false);
    expect(validatePreference("timeline.snap_targets", "edges").ok).toBe(false);
    // A default is a copy: changing one never changes the schema's.
    const a = defaultPreferences()["timeline.snap_targets"];
    expect(a).not.toBe(defaultPreferences()["timeline.snap_targets"]);
  });

  it("bounds the numbers", () => {
    expect(validatePreference("editing.undo_steps", 2).ok).toBe(true);
    expect(validatePreference("editing.undo_steps", 0).ok).toBe(false);
    expect(validatePreference("editing.undo_steps", 10_001).ok).toBe(false);
    expect(validatePreference("editing.nudge", 101).ok).toBe(false);
    expect(validatePreference("timeline.snap_distance", 1).ok).toBe(false);
    expect(validatePreference("timeline.snap_distance", 32).ok).toBe(true);
    expect(validatePreference("timeline.follow", "jump").ok).toBe(false);
  });

  it("writes a changed set to the file and reads it back", () => {
    const values = { ...defaultPreferences(), "timeline.snap_targets": ["edges"] as const };
    const text = serializePreferences(values);
    expect(JSON.parse(text).app["timeline.snap_targets"]).toEqual(["edges"]);
    expect(loadPreferences(text).values["timeline.snap_targets"]).toEqual(["edges"]);
  });
});

describe("export presets and file names (WP-2G.04)", () => {
  it("defaults to the last preset used and the project's name", () => {
    const values = defaultPreferences();
    expect(values["export.preset"]).toBe("");
    expect(values["export.last_preset"]).toBe("");
    expect(values["export.filename"]).toBe("{project}");
  });

  it("takes a preset id and a file name pattern, and refuses what a file name cannot hold", () => {
    expect(validatePreference("export.preset", "youtube-1080p").ok).toBe(true);
    expect(validatePreference("export.preset", "").ok).toBe(true);
    expect(validatePreference("export.preset", "YouTube 1080p").ok).toBe(false);
    expect(validatePreference("export.preset", "a".repeat(65)).ok).toBe(false);
    expect(validatePreference("export.filename", "{project}_{date}").ok).toBe(true);
    expect(validatePreference("export.filename", "a/b").ok).toBe(false);
    expect(validatePreference("export.filename", "a:b").ok).toBe(false);
    expect(validatePreference("export.filename", "a\nb").ok).toBe(false);
    expect(validatePreference("export.filename", "").ok).toBe(false);
    expect(validatePreference("export.filename", 3).ok).toBe(false);
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

describe("agents and notifications (WP-1J.05)", () => {
  it("defaults to no extra folders, the standard tool set and connection notices on", () => {
    const values = defaultPreferences();
    expect(values["agents.allowed_folders"]).toEqual([]);
    expect(values["agents.client_profile"]).toBe("standard");
    expect(values["notify.agent_connected"]).toBe(true);
  });

  it("takes a short list of distinct absolute folders", () => {
    expect(validatePreference("agents.allowed_folders", ["C:\\Footage", "/media/b"]).ok).toBe(true);
    expect(validatePreference("agents.allowed_folders", []).ok).toBe(true);
    for (const bad of [
      "C:\\Footage",
      ["relative/dir"],
      [""],
      ["/a", "/a"],
      [1],
      ["/a\u0000b"],
      Array.from({ length: MAX_AGENT_FOLDERS + 1 }, (_, i) => `/f${i}`),
    ]) {
      expect(validatePreference("agents.allowed_folders", bad).ok, JSON.stringify(bad)).toBe(false);
    }
    expect(validatePreference("agents.client_profile", "full").ok).toBe(true);
    expect(validatePreference("agents.client_profile", "huge").ok).toBe(false);
  });

  it("lets only main write the folders, and never exports or imports them", () => {
    expect(isRendererWritable("agents.allowed_folders")).toBe(false);
    expect(isRendererWritable("agents.client_profile")).toBe(true);
    expect(isRendererWritable("notify.agent_connected")).toBe(true);
    expect(isRendererWritable("ghost")).toBe(false);
    const values = { ...defaultPreferences(), "agents.allowed_folders": ["D:\\Footage"] };
    const text = serializePreferences(values);
    expect(JSON.parse(text).machine).toEqual({ "agents.allowed_folders": ["D:\\Footage"] });
    expect(loadPreferences(text).values["agents.allowed_folders"]).toEqual(["D:\\Footage"]);
    expect(exportPreferences(values).text).not.toContain("allowed_folders");
    const parsed = parseImport(
      JSON.stringify({ version: 1, app: { "agents.allowed_folders": ["C:\\Users"] } }),
    );
    expect(parsed.ok && parsed.applied).toEqual({});
    expect(parsed.ok && parsed.skipped[0]?.reason).toBe("belongs to this computer");
  });

  it("compares folder lists by their items", () => {
    expect(samePreferenceValue(["/a", "/b"], ["/a", "/b"])).toBe(true);
    expect(samePreferenceValue(["/a", "/b"], ["/b", "/a"])).toBe(false);
    expect(samePreferenceValue([], [])).toBe(true);
  });
});

describe("keyboard shortcuts (WP-2H.05)", () => {
  it("defaults to the NLE preset and no changes", () => {
    const defaults = defaultPreferences();
    expect(defaults["shortcuts.preset"]).toBe("nle");
    expect(defaults["shortcuts.bindings"]).toEqual({});
  });

  it("takes binding ids with the keys they now have, and an empty list for a removal", () => {
    const ok = validatePreference("shortcuts.bindings", {
      "clip.split": ["Mod+K", "S"],
      "palette.open": [],
      "workspace.switch_workspace:short-form": ["Mod+K Mod+S"],
    });
    expect(ok.ok).toBe(true);
    expect(validatePreference("shortcuts.preset", "capcut").ok).toBe(true);
    expect(validatePreference("shortcuts.preset", "emacs").ok).toBe(false);
  });

  it("refuses what is not a key list, and drops a repeated key", () => {
    for (const bad of [
      { "has space": ["S"] },
      { "clip.split": "S" },
      { "clip.split": [1] },
      { "clip.split": ["Mod+K Mod+S Mod+X"] },
      { "clip.split": ["S", "S", "S", "S", "S"] },
      [],
    ]) {
      expect(validatePreference("shortcuts.bindings", bad).ok, JSON.stringify(bad)).toBe(false);
    }
    const dup = validatePreference("shortcuts.bindings", { "clip.split": ["S", "S"] });
    expect(dup).toEqual({ ok: true, value: { "clip.split": ["S"] } });
  });

  it("compares key lists by value, so an unchanged list is not written", () => {
    expect(samePreferenceValue({ a: ["S"] }, { a: ["S"] })).toBe(true);
    expect(samePreferenceValue({ a: ["S"] }, { a: ["K"] })).toBe(false);
    const values = defaultPreferences();
    expect(JSON.parse(serializePreferences(values))).toEqual({
      version: 1,
      app: {},
      machine: {},
    });
  });

  it("exports only the shortcuts when asked, and imports only them", () => {
    const values = {
      ...defaultPreferences(),
      "appearance.theme": "light" as const,
      "shortcuts.preset": "capcut" as const,
      "shortcuts.bindings": { "clip.split": ["Mod+K"] },
    };
    const { text, count } = exportPreferences(values, defaultPreferences(), "shortcuts.");
    expect(count).toBe(2);
    expect(JSON.parse(text)).toEqual({
      version: 1,
      app: { "shortcuts.preset": "capcut", "shortcuts.bindings": { "clip.split": ["Mod+K"] } },
    });
    const parsed = parseImport(
      JSON.stringify({
        version: 1,
        app: { "appearance.theme": "light", "shortcuts.bindings": { "item.delete": ["Delete"] } },
      }),
      "shortcuts.",
    );
    expect(parsed.ok).toBe(true);
    if (!parsed.ok) return;
    // The theme is not a shortcut; what the file leaves out goes back to its default.
    expect(parsed.applied).toEqual({
      "shortcuts.preset": "nle",
      "shortcuts.bindings": { "item.delete": ["Delete"] },
    });
    expect(parsed.skipped.map((s) => s.key)).toEqual(["appearance.theme"]);
  });
});

describe("appearance: interface scale, density and reduce motion (WP-2H.09)", () => {
  it("defaults to 100%, Standard and the system's motion setting", () => {
    const values = defaultPreferences();
    expect(values["appearance.scale"]).toBe(100);
    expect(values["appearance.density"]).toBe("standard");
    expect(values["appearance.reduce_motion"]).toBe("system");
  });

  it("offers 90, 100, 110, 125 and 150% and nothing between", () => {
    expect([...INTERFACE_SCALES]).toEqual([90, 100, 110, 125, 150]);
    for (const scale of INTERFACE_SCALES) {
      expect(validatePreference("appearance.scale", scale).ok).toBe(true);
    }
    for (const bad of [0, 80, 120, 200, 1.25, "125", null]) {
      expect(validatePreference("appearance.scale", bad).ok, String(bad)).toBe(false);
    }
  });

  it("turns a scale into the window's zoom factor", () => {
    expect(INTERFACE_SCALES.map(zoomFactorFor)).toEqual([0.9, 1, 1.1, 1.25, 1.5]);
  });

  it("accepts the two densities and the three motion choices only", () => {
    for (const density of DENSITY_CHOICES) {
      expect(validatePreference("appearance.density", density).ok).toBe(true);
    }
    for (const motion of REDUCE_MOTION_CHOICES) {
      expect(validatePreference("appearance.reduce_motion", motion).ok).toBe(true);
    }
    expect(validatePreference("appearance.density", "compact").ok).toBe(false);
    expect(validatePreference("appearance.reduce_motion", true).ok).toBe(false);
  });

  it("names the root attributes: Standard is `compact`, and the system has no `data-motion`", () => {
    expect(densityAttribute("standard")).toBe("compact");
    expect(densityAttribute("comfortable")).toBe("comfortable");
    expect(motionAttribute("system")).toBeUndefined();
    expect(motionAttribute("on")).toBe("reduce");
    expect(motionAttribute("off")).toBe("full");
  });

  it("exports and imports the three with the other app preferences", () => {
    const values = {
      ...defaultPreferences(),
      "appearance.scale": 125 as const,
      "appearance.density": "comfortable" as const,
      "appearance.reduce_motion": "on" as const,
    };
    const { text, count } = exportPreferences(values, defaultPreferences());
    expect(count).toBe(3);
    const parsed = parseImport(text);
    expect(parsed.ok).toBe(true);
    if (!parsed.ok) return;
    expect(parsed.applied).toEqual({
      "appearance.scale": 125,
      "appearance.density": "comfortable",
      "appearance.reduce_motion": "on",
    });
  });
});
