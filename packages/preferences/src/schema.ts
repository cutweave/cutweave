// SPDX-License-Identifier: GPL-3.0-or-later
// The preferences schema (WP-1M.11, arch §16): every key the Settings dialog can change, with its
// type, default and range, and the pure functions that read, migrate, validate and export a
// `preferences.json`. Electron main (the store, the IPC) and the editor (the dialog, the
// registry test) import this one module, so they cannot disagree. No dependency, no Node or DOM
// API: it also runs in the preload and in tests.
//
// Where each key is documented: docs/reference/settings.md (a test checks that every key here
// has a row there). Secrets never belong here: keys go to the OS keychain (arch §13).

/** The file's schema version. A change to a key's meaning or shape adds a migration below. */
export const SCHEMA_VERSION = 1;

export const THEME_CHOICES = ["dark", "light", "system"] as const;
export type ThemeChoice = (typeof THEME_CHOICES)[number];
/** What a window can show: `system` resolves to one of these. */
export type ResolvedTheme = Exclude<ThemeChoice, "system">;

/** The theme a window shows for the person's choice; `system` follows the OS. */
export function resolveTheme(choice: ThemeChoice, systemIsDark: boolean): ResolvedTheme {
  if (choice === "system") return systemIsDark ? "dark" : "light";
  return choice;
}

export const STARTUP_CHOICES = ["empty", "last"] as const;
export type StartupChoice = (typeof STARTUP_CHOICES)[number];

/** "Workspace for new projects": the one used last, or always Long-form or Short-form (WP-2H.01). */
export const WORKSPACE_CHOICES = ["last", "long-form", "short-form"] as const;
export type WorkspaceChoice = (typeof WORKSPACE_CHOICES)[number];

/** "Save at least every": 30 s, 1 min, 2 min, 5 min. */
export const AUTOSAVE_INTERVALS_S = [30, 60, 120, 300] as const;
/** "Backups to keep". */
export const AUTOSAVE_BACKUPS = [5, 10, 20] as const;

/** Where File > Export… proposes to save: next to the project, where the last export went, or a chosen folder. */
export const EXPORT_FOLDER_CHOICES = ["project", "last", "chosen"] as const;
export type ExportFolderChoice = (typeof EXPORT_FOLDER_CHOICES)[number];
export const EXPORT_QUALITIES = ["low", "medium", "high"] as const;
export type ExportQualityChoice = (typeof EXPORT_QUALITIES)[number];
/** Hardware encoders: let the engine pick one, or software only. */
export const EXPORT_HW_CHOICES = ["auto", "off"] as const;
export type ExportHwChoice = (typeof EXPORT_HW_CHOICES)[number];
const MIB = 1_048_576;
const GIB = 1_073_741_824;

/** "Thumbnails and filmstrips" cache limits (WP-1D.08), in bytes: 512 MiB to 25 GiB. */
export const THUMBNAIL_LIMITS = [512 * MIB, 1 * GIB, 2 * GIB, 5 * GIB, 10 * GIB, 25 * GIB] as const;
/** "Waveforms" cache limits, in bytes: 128 MiB to 2 GiB. */
export const WAVEFORM_LIMITS = [128 * MIB, 256 * MIB, 512 * MIB, 1 * GIB, 2 * GIB] as const;
/** "Memory for decoded frames": 0 is Auto (the smaller of 1 GiB and a quarter of the RAM). */
export const FRAME_MEMORY_CHOICES = [0, 512 * MIB, 1 * GIB, 2 * GIB, 4 * GIB] as const;
/** "Hardware decoding". */
export const HW_DECODE_CHOICES = ["auto", "off"] as const;
export type HwDecodeChoice = (typeof HW_DECODE_CHOICES)[number];

/** "Don't ask again" choices dialogs remember, by dialog id. */
export type DialogChoices = Record<string, string>;

/** The tool set of a copied agent client setup (`cutweave mcp --profile`, WP-1J.05). */
export const CLIENT_PROFILES = ["minimal", "standard", "full"] as const;
export type ClientProfile = (typeof CLIENT_PROFILES)[number];

/** The most folders "Folders agents can use" holds (the engine takes no more, WP-1J.05). */
export const MAX_AGENT_FOLDERS = 64;

/** Every preference key and the type of its value. */
export interface PreferenceValues {
  "general.startup": StartupChoice;
  /** `""` is "not chosen": main's context default (Documents/Cutweave) applies. */
  "general.new_project_folder": string;
  "general.recent_count": number;
  "general.workspace": WorkspaceChoice;
  "general.dialog_choices": DialogChoices;
  "appearance.theme": ThemeChoice;
  "saving.autosave": boolean;
  "saving.interval": (typeof AUTOSAVE_INTERVALS_S)[number];
  "saving.backups": (typeof AUTOSAVE_BACKUPS)[number];
  // Export (WP-1M.13)
  "export.folder": ExportFolderChoice;
  /** The folder "chosen" means; `""` is "not chosen yet" (the project folder applies). */
  "export.folder_path": string;
  /** The folder the last export was saved to from the dialog; `""` is none yet. */
  "export.last_folder": string;
  "export.quality": ExportQualityChoice;
  "export.hw": ExportHwChoice;
  /** `""` is the OS cache folder. Main passes the value to the engine as `--cache-dir`. */
  "cache.folder": string;
  "cache.thumbnails_limit": (typeof THUMBNAIL_LIMITS)[number];
  "cache.waveforms_limit": (typeof WAVEFORM_LIMITS)[number];
  "performance.hw_decode": HwDecodeChoice;
  "performance.frame_memory": (typeof FRAME_MEMORY_CHOICES)[number];
  /** Notifications (WP-1J.05): a toast when an agent connects or disconnects. */
  "notify.agent_connected": boolean;
  /**
   * Agents and MCP (WP-1J.05): folders agents may use besides the open project's, as main
   * stored them (resolved, absolute). This computer's: never exported or imported, and only
   * main writes it (its folder picker), so the renderer never names a folder.
   */
  "agents.allowed_folders": readonly string[];
  "agents.client_profile": ClientProfile;
  // Media and import (WP-1M.14)
  /** Show a video's frames while the pointer moves across its card in the bin. */
  "media.hover_scrub": boolean;
}

export type PreferenceKey = keyof PreferenceValues;
export type PreferenceMap = { [K in PreferenceKey]: PreferenceValues[K] };
/** A change or a partial set of values. */
export type PreferenceChanges = { [K in PreferenceKey]?: PreferenceValues[K] };

/** App preferences are the person's; machine ones belong to this installation (never exported). */
export type PreferenceScope = "app" | "machine";

export type PreferenceDef =
  | { type: "boolean"; scope: PreferenceScope; default: boolean }
  | {
      type: "enum";
      scope: PreferenceScope;
      options: readonly (string | number)[];
      default: string | number;
    }
  | { type: "int"; scope: PreferenceScope; min: number; max: number; default: number }
  /** An absolute folder or file path, or `""` for "not chosen". */
  | { type: "path"; scope: PreferenceScope; default: string }
  /** A short map of remembered choices: ids to short words. */
  | { type: "record"; scope: PreferenceScope; default: Record<string, string> }
  /**
   * A list of distinct absolute paths, at most `max`. `writer: "main"`: only main changes it
   * (through its own dialogs); `prefs:set` from the renderer is refused.
   */
  | {
      type: "paths";
      scope: PreferenceScope;
      default: readonly string[];
      max: number;
      writer?: "main";
    };

export const RECENT_COUNT_MAX = 25;
const MAX_PATH_LENGTH = 4096;
const MAX_RECORD_ENTRIES = 64;
const RECORD_KEY = /^[a-z0-9][a-z0-9._-]{0,63}$/;
const MAX_RECORD_VALUE = 64;

export const PREFERENCES: { readonly [K in PreferenceKey]: PreferenceDef } = {
  "general.startup": { type: "enum", scope: "app", options: STARTUP_CHOICES, default: "empty" },
  "general.new_project_folder": { type: "path", scope: "app", default: "" },
  "general.recent_count": { type: "int", scope: "app", min: 0, max: RECENT_COUNT_MAX, default: 10 },
  "general.workspace": { type: "enum", scope: "app", options: WORKSPACE_CHOICES, default: "last" },
  "general.dialog_choices": { type: "record", scope: "app", default: {} },
  "appearance.theme": { type: "enum", scope: "app", options: THEME_CHOICES, default: "dark" },
  "saving.autosave": { type: "boolean", scope: "app", default: true },
  "saving.interval": { type: "enum", scope: "app", options: AUTOSAVE_INTERVALS_S, default: 30 },
  "saving.backups": { type: "enum", scope: "app", options: AUTOSAVE_BACKUPS, default: 10 },
  // Export (WP-1M.13)
  "export.folder": {
    type: "enum",
    scope: "app",
    options: EXPORT_FOLDER_CHOICES,
    default: "project",
  },
  "export.folder_path": { type: "path", scope: "app", default: "" },
  "export.last_folder": { type: "path", scope: "app", default: "" },
  "export.quality": { type: "enum", scope: "app", options: EXPORT_QUALITIES, default: "medium" },
  "export.hw": { type: "enum", scope: "app", options: EXPORT_HW_CHOICES, default: "auto" },
  // This computer (WP-1D.08): never exported, applied to the engine by the app.
  "cache.folder": { type: "path", scope: "machine", default: "" },
  "cache.thumbnails_limit": {
    type: "enum",
    scope: "machine",
    options: THUMBNAIL_LIMITS,
    default: 2 * GIB,
  },
  "cache.waveforms_limit": {
    type: "enum",
    scope: "machine",
    options: WAVEFORM_LIMITS,
    default: 512 * MIB,
  },
  "performance.hw_decode": {
    type: "enum",
    scope: "machine",
    options: HW_DECODE_CHOICES,
    default: "auto",
  },
  "performance.frame_memory": {
    type: "enum",
    scope: "machine",
    options: FRAME_MEMORY_CHOICES,
    default: 0,
  },
  // Notifications and Agents and MCP (WP-1J.05).
  "notify.agent_connected": { type: "boolean", scope: "app", default: true },
  "agents.allowed_folders": {
    type: "paths",
    scope: "machine",
    default: [],
    max: MAX_AGENT_FOLDERS,
    writer: "main",
  },
  "agents.client_profile": {
    type: "enum",
    scope: "app",
    options: CLIENT_PROFILES,
    default: "standard",
  },
  // Media and import (WP-1M.14)
  "media.hover_scrub": { type: "boolean", scope: "app", default: true },
};

export const PREFERENCE_KEYS = Object.keys(PREFERENCES) as PreferenceKey[];

export function isPreferenceKey(key: unknown): key is PreferenceKey {
  return typeof key === "string" && Object.hasOwn(PREFERENCES, key);
}

/**
 * Whether the editor may write `key` (`prefs:set`). Keys only main writes are changed through
 * main's own channels: "Folders agents can use" adds a folder with main's folder picker, so a
 * renderer never names a path agents may then use (WP-1J.05). Resetting them is allowed.
 */
export function isRendererWritable(key: unknown): boolean {
  if (!isPreferenceKey(key)) return false;
  const def = PREFERENCES[key];
  return !(def.type === "paths" && def.writer === "main");
}

export type Validation<T = unknown> = { ok: true; value: T } | { ok: false; reason: string };

const isPlainObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** POSIX `/x`, Windows `C:\x`, `C:/x` or UNC `\\host\share`. */
function isAbsolutePath(value: string): boolean {
  return /^(?:[\\/]|[A-Za-z]:[\\/])/.test(value);
}

function validateAgainst(def: PreferenceDef, value: unknown): Validation {
  switch (def.type) {
    case "boolean":
      return typeof value === "boolean"
        ? { ok: true, value }
        : { ok: false, reason: "expected true or false" };
    case "enum":
      return def.options.includes(value as string | number)
        ? { ok: true, value }
        : { ok: false, reason: `expected one of ${def.options.join(", ")}` };
    case "int":
      if (typeof value !== "number" || !Number.isInteger(value)) {
        return { ok: false, reason: "expected a whole number" };
      }
      return value >= def.min && value <= def.max
        ? { ok: true, value }
        : { ok: false, reason: `expected ${def.min} to ${def.max}` };
    case "path":
      if (typeof value !== "string") return { ok: false, reason: "expected a path" };
      if (value === "") return { ok: true, value };
      if (value.length > MAX_PATH_LENGTH || value.includes("\0")) {
        return { ok: false, reason: "not a usable path" };
      }
      return isAbsolutePath(value)
        ? { ok: true, value }
        : { ok: false, reason: "expected an absolute path" };
    case "paths": {
      if (!Array.isArray(value)) return { ok: false, reason: "expected a list of folders" };
      if (value.length > def.max) return { ok: false, reason: `at most ${def.max} folders` };
      const clean: string[] = [];
      for (const item of value) {
        if (
          typeof item !== "string" ||
          item === "" ||
          item.length > MAX_PATH_LENGTH ||
          item.includes("\0") ||
          !isAbsolutePath(item)
        ) {
          return { ok: false, reason: "expected absolute folder paths" };
        }
        if (clean.includes(item)) return { ok: false, reason: `"${item}" is listed twice` };
        clean.push(item);
      }
      return { ok: true, value: clean };
    }
    case "record": {
      if (!isPlainObject(value)) return { ok: false, reason: "expected a list of choices" };
      const entries = Object.entries(value);
      if (entries.length > MAX_RECORD_ENTRIES) return { ok: false, reason: "too many choices" };
      const clean: Record<string, string> = {};
      for (const [id, choice] of entries) {
        if (
          !RECORD_KEY.test(id) ||
          typeof choice !== "string" ||
          choice.length > MAX_RECORD_VALUE
        ) {
          return { ok: false, reason: `"${id}" is not a valid choice` };
        }
        clean[id] = choice;
      }
      return { ok: true, value: clean };
    }
  }
}

/** Checks one value against its key's type and range. Never throws. */
export function validatePreference(key: unknown, value: unknown): Validation {
  if (!isPreferenceKey(key)) return { ok: false, reason: "unknown setting" };
  return validateAgainst(PREFERENCES[key], value);
}

/** The defaults, with the machine's own for keys whose default depends on it. */
export function defaultPreferences(overrides: PreferenceChanges = {}): PreferenceMap {
  const out: Record<string, unknown> = {};
  for (const key of PREFERENCE_KEYS) {
    const def = PREFERENCES[key];
    out[key] =
      def.type === "record"
        ? { ...def.default }
        : def.type === "paths"
          ? [...def.default]
          : def.default;
  }
  for (const [key, value] of Object.entries(overrides)) {
    if (validatePreference(key, value).ok) out[key] = value;
  }
  return out as unknown as PreferenceMap;
}

export function samePreferenceValue(a: unknown, b: unknown): boolean {
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((item, i) => item === b[i]);
  }
  if (isPlainObject(a) && isPlainObject(b)) {
    const ka = Object.keys(a);
    return (
      ka.length === Object.keys(b).length && ka.every((k) => Object.hasOwn(b, k) && a[k] === b[k])
    );
  }
  return a === b;
}

// ---- the file ---------------------------------------------------------------------------------

/**
 * Migrations, one per version step: `MIGRATIONS[n]` turns a version-n document into version
 * n + 1. Version 0 is a file with no `version`: one flat map of key to value, as a hand-written
 * file would have it, before the app and machine groups existed.
 */
const MIGRATIONS: Record<number, (doc: Record<string, unknown>) => Record<string, unknown>> = {
  0: (doc) => {
    const app: Record<string, unknown> = {};
    const machine: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(doc)) {
      if (key === "version") continue;
      const target = isPreferenceKey(key) && PREFERENCES[key].scope === "machine" ? machine : app;
      target[key] = value;
    }
    return { version: 1, app, machine };
  },
};

export interface LoadedPreferences {
  /** Every key, from the file where it was valid and the default elsewhere. */
  values: PreferenceMap;
  /** One line per thing that was dropped; empty for a clean file. */
  warnings: string[];
  /** The file's version when it needed migrating, else null. */
  migratedFrom: number | null;
  /** The file should be written again (it was damaged, partly invalid, old or newer). */
  rewrite: boolean;
}

/**
 * Reads the text of a `preferences.json`. Never throws and never blocks startup: bad JSON, an
 * unknown key, a wrong type or an out-of-range value each fall back to that key's default and
 * add a warning, and `rewrite` asks the caller to write the file again, clean.
 */
export function loadPreferences(
  text: string | null,
  contextDefaults: PreferenceChanges = {},
): LoadedPreferences {
  const values = defaultPreferences(contextDefaults);
  const warnings: string[] = [];
  if (text === null) return { values, warnings, migratedFrom: null, rewrite: false };
  let doc: unknown;
  try {
    doc = JSON.parse(text);
  } catch {
    warnings.push("preferences.json is damaged and was ignored");
    return { values, warnings, migratedFrom: null, rewrite: true };
  }
  if (!isPlainObject(doc)) {
    warnings.push("preferences.json is not an object and was ignored");
    return { values, warnings, migratedFrom: null, rewrite: true };
  }
  let version = typeof doc.version === "number" && Number.isInteger(doc.version) ? doc.version : 0;
  let migratedFrom: number | null = null;
  let current: Record<string, unknown> = doc;
  let rewrite = false;
  if (version > SCHEMA_VERSION) {
    warnings.push(
      `preferences.json is from a newer Cutweave (version ${version}); using what is known`,
    );
    rewrite = true;
  }
  while (version < SCHEMA_VERSION) {
    const step = MIGRATIONS[version];
    if (step === undefined) break;
    migratedFrom ??= version;
    current = step(current);
    version += 1;
    rewrite = true;
  }
  for (const group of ["app", "machine"] as const) {
    const entries = current[group];
    if (entries === undefined) continue;
    if (!isPlainObject(entries)) {
      warnings.push(`"${group}" is not an object and was ignored`);
      rewrite = true;
      continue;
    }
    for (const [key, value] of Object.entries(entries)) {
      if (!isPreferenceKey(key)) {
        warnings.push(`unknown setting "${key}" was dropped`);
        rewrite = true;
        continue;
      }
      if (PREFERENCES[key].scope !== group) {
        warnings.push(`"${key}" is in the wrong group and was dropped`);
        rewrite = true;
        continue;
      }
      const checked = validateAgainst(PREFERENCES[key], value);
      if (!checked.ok) {
        warnings.push(`"${key}" ${checked.reason}; using the default`);
        rewrite = true;
        continue;
      }
      (values as Record<string, unknown>)[key] = checked.value;
    }
  }
  return { values, warnings, migratedFrom, rewrite };
}

/** The file's text: only values that differ from their defaults, so a new default reaches everyone. */
export function serializePreferences(
  values: PreferenceMap,
  defaults: PreferenceMap = defaultPreferences(),
): string {
  const app: Record<string, unknown> = {};
  const machine: Record<string, unknown> = {};
  for (const key of PREFERENCE_KEYS) {
    if (samePreferenceValue(values[key], defaults[key])) continue;
    (PREFERENCES[key].scope === "machine" ? machine : app)[key] = values[key];
  }
  return `${JSON.stringify({ version: SCHEMA_VERSION, app, machine }, null, 2)}\n`;
}

// ---- export and import ------------------------------------------------------------------------

/**
 * "Export settings…": the app preferences that differ from their defaults, with the schema
 * version. Never machine values, keys, recent projects or window state.
 */
export function exportPreferences(
  values: PreferenceMap,
  defaults: PreferenceMap = defaultPreferences(),
): { text: string; count: number } {
  const app: Record<string, unknown> = {};
  for (const key of PREFERENCE_KEYS) {
    if (PREFERENCES[key].scope !== "app") continue;
    if (samePreferenceValue(values[key], defaults[key])) continue;
    app[key] = values[key];
  }
  const text = `${JSON.stringify({ version: SCHEMA_VERSION, app }, null, 2)}\n`;
  return { text, count: Object.keys(app).length };
}

export interface SkippedSetting {
  key: string;
  reason: string;
}

export type ParsedImport =
  | { ok: true; applied: PreferenceChanges; skipped: SkippedSetting[] }
  | { ok: false; reason: "unreadable" | "not-settings" | "newer" };

/** "Import settings…": validates every key; the valid ones are returned, the rest are listed. */
export function parseImport(text: string): ParsedImport {
  let doc: unknown;
  try {
    doc = JSON.parse(text);
  } catch {
    return { ok: false, reason: "unreadable" };
  }
  if (!isPlainObject(doc) || typeof doc.version !== "number" || !isPlainObject(doc.app)) {
    return { ok: false, reason: "not-settings" };
  }
  if (doc.version > SCHEMA_VERSION) return { ok: false, reason: "newer" };
  const applied: Record<string, unknown> = {};
  const skipped: SkippedSetting[] = [];
  for (const [key, value] of Object.entries(doc.app)) {
    if (!isPreferenceKey(key)) {
      skipped.push({ key, reason: "unknown setting" });
    } else if (PREFERENCES[key].scope !== "app") {
      skipped.push({ key, reason: "belongs to this computer" });
    } else {
      const checked = validateAgainst(PREFERENCES[key], value);
      if (checked.ok) applied[key] = checked.value;
      else skipped.push({ key, reason: checked.reason });
    }
  }
  if (isPlainObject(doc.machine)) {
    for (const key of Object.keys(doc.machine)) {
      skipped.push({ key, reason: "belongs to this computer" });
    }
  }
  return { ok: true, applied: applied as PreferenceChanges, skipped };
}
