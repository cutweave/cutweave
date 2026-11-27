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
/** "Proxies" cache limits (WP-2F.03b), in bytes: 10 GiB to 500 GiB. */
export const PROXY_LIMITS = [
  10 * GIB,
  25 * GIB,
  50 * GIB,
  100 * GIB,
  250 * GIB,
  500 * GIB,
] as const;
/** "Proxy jobs at once" (WP-2F.03b). */
export const PROXY_JOB_CHOICES = [1, 2, 3, 4] as const;
/** "Proxy size" (WP-2F.03b): the longest side of a proxy picture, in pixels. */
export const PROXY_SIZES = [540, 720, 960] as const;
/** "Proxies for new projects" (WP-2F.03b, ADR-0027): never, for video that is slow to scrub, always. */
export const PROXY_POLICIES = ["off", "auto", "all"] as const;
export type ProxyPolicyChoice = (typeof PROXY_POLICIES)[number];
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

/**
 * The frame rate a new image sequence gets (WP-2F.05c, ADR-0035): the frame rate of the sequence
 * the editor shows, or a fixed one. Stored on the asset at import; changing the setting never
 * changes an imported sequence.
 */
export const IMAGE_SEQUENCE_FPS_CHOICES = [
  "sequence",
  "23.976",
  "24",
  "25",
  "29.97",
  "30",
  "50",
  "59.94",
  "60",
] as const;
export type ImageSequenceFps = (typeof IMAGE_SEQUENCE_FPS_CHOICES)[number];

/** The History panel lists its steps newest first or oldest first (#171). */
export const HISTORY_ORDERS = ["oldest", "newest"] as const;
export type HistoryOrder = (typeof HISTORY_ORDERS)[number];

// Editing and Timeline (WP-2A.13)
/** Media dropped onto a clip outside the main track replaces what it covers, or pushes it later. */
export const DROP_MODES = ["overwrite", "insert"] as const;
export type DropMode = (typeof DROP_MODES)[number];
/** The most frames one nudge moves, and the undo steps kept per person or agent. */
export const NUDGE_MAX = 100;
export const UNDO_STEPS_MAX = 10_000;
/** What a dragged clip can snap to (Settings > Timeline > Snap to). */
export const SNAP_TARGETS = ["playhead", "edges", "markers", "beats"] as const;
export type SnapTarget = (typeof SNAP_TARGETS)[number];
/** The snap distance, in pixels. */
export const SNAP_DISTANCE_MIN = 2;
export const SNAP_DISTANCE_MAX = 32;
/** How the timeline follows the playhead while playing. */
export const FOLLOW_MODES = ["page", "smooth", "off"] as const;
export type FollowModeChoice = (typeof FOLLOW_MODES)[number];
/** What video clips show on the timeline. */
export const THUMBNAIL_MODES = ["filmstrip", "first", "off"] as const;
export type ThumbnailModeChoice = (typeof THUMBNAIL_MODES)[number];
/** The height a new track gets. */
export const TRACK_HEIGHTS = ["small", "medium", "large"] as const;
export type TrackHeightChoice = (typeof TRACK_HEIGHTS)[number];
/** What the mouse wheel does over the timeline; Ctrl/Cmd+wheel does the other. */
export const WHEEL_MODES = ["scroll", "zoom"] as const;
export type WheelModeChoice = (typeof WHEEL_MODES)[number];
/** How the ruler, the transport and the Inspector show time. */
export const TIME_DISPLAYS = ["timecode", "frames", "seconds"] as const;
export type TimeDisplayChoice = (typeof TIME_DISPLAYS)[number];

/** "Keyboard shortcuts" presets (WP-2H.05): the NLE-style keys, or CapCut's. */
export const SHORTCUT_PRESETS = ["nle", "capcut"] as const;
export type ShortcutPreset = (typeof SHORTCUT_PRESETS)[number];

/**
 * Your shortcuts as they differ from the preset (WP-2H.05): a binding id (a command, or
 * `command:argument` for one that carries a fixed argument) to the keys it now has. Each key is
 * a stroke or a chord of two strokes, spelled `Mod+Shift+Z` or `Mod+K Mod+S`. An empty list
 * removes the binding. Only these differences are stored, so a preset fixed in an update
 * still reaches people who changed other keys.
 */
export type ShortcutOverrides = Record<string, string[]>;

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
  // Export (WP-2G.04)
  /** The preset File > Export… starts at; `""` is "the last one used". */
  "export.preset": string;
  /** The preset of the last export started from the dialog; `""` is none yet. */
  "export.last_preset": string;
  /** The file name pattern: `{project}`, `{sequence}`, `{date}` and `{preset}` are filled in. */
  "export.filename": string;
  /** `""` is the OS cache folder. Main passes the value to the engine as `--cache-dir`. */
  "cache.folder": string;
  "cache.thumbnails_limit": (typeof THUMBNAIL_LIMITS)[number];
  "cache.waveforms_limit": (typeof WAVEFORM_LIMITS)[number];
  "cache.proxies_limit": (typeof PROXY_LIMITS)[number];
  "performance.hw_decode": HwDecodeChoice;
  "performance.frame_memory": (typeof FRAME_MEMORY_CHOICES)[number];
  /** How many proxies are made at the same time (one while an export runs). */
  "performance.proxy_jobs": (typeof PROXY_JOB_CHOICES)[number];
  /** Notifications (WP-1J.05): a toast when an agent connects or disconnects. */
  "notify.agent_connected": boolean;
  /**
   * Agents and MCP (WP-1J.05): folders agents may use besides the open project's, as main
   * stored them (resolved, absolute). This computer's: never exported or imported, and only
   * main writes it (its folder picker), so the renderer never names a folder.
   */
  "agents.allowed_folders": readonly string[];
  "agents.client_profile": ClientProfile;
  // Editing (WP-2F.05b, ADR-0035)
  /** Seconds a still image gets in new projects when it is placed without a length. */
  "editing.still_duration": number;
  // Media and import (WP-1M.14)
  /** Show a video's frames while the pointer moves across its card in the bin. */
  "media.hover_scrub": boolean;
  /** Folders in an import keep their directories as bin folders (WP-2F.09). */
  "media.keep_structure": boolean;
  /** Imports copy each new file into the project's `media/` folder first (WP-2F.04c). */
  "media.copy_on_import": boolean;
  /** The frame rate of a new image sequence (WP-2F.05c); `"sequence"` is the sequence's own. */
  "media.image_sequence_fps": ImageSequenceFps;
  /** The proxy policy a new project starts with (WP-2F.03b, ADR-0027). */
  "media.proxy_policy": ProxyPolicyChoice;
  /** The longest side of the proxies this computer makes, in pixels. */
  "media.proxy_size": (typeof PROXY_SIZES)[number];
  // Editing (WP-2A.13)
  /** A video placed on the timeline brings its sound, linked to it. */
  "editing.link_av": boolean;
  /** Media dropped onto a clip outside the main track. */
  "editing.drop_mode": DropMode;
  /** Frames `,` and `.` move the selected clips (Shift: ten times as far). */
  "editing.nudge": number;
  /** Undo steps the engine keeps per person or agent (`edit.set_history_limit`). */
  "editing.undo_steps": number;
  // Timeline (WP-2A.13)
  /** Snapping is on when a project opens (the toolbar's toggle is for the session). */
  "timeline.snap": boolean;
  "timeline.snap_targets": readonly SnapTarget[];
  /** Snap distance in pixels. */
  "timeline.snap_distance": number;
  "timeline.follow": FollowModeChoice;
  /** The main video track of a new sequence is magnetic. */
  "timeline.magnetic_main": boolean;
  "timeline.thumbnails": ThumbnailModeChoice;
  "timeline.waveforms": boolean;
  "timeline.track_height": TrackHeightChoice;
  "timeline.wheel": WheelModeChoice;
  "timeline.time_display": TimeDisplayChoice;
  /** Drop-frame timecode at 29.97 and 59.94 fps. */
  "timeline.drop_frame": boolean;
  /** The History panel's sort order (#171), the person's, not the project's. */
  "history.order": HistoryOrder;
  // Keyboard shortcuts (WP-2H.05)
  "shortcuts.preset": ShortcutPreset;
  "shortcuts.bindings": ShortcutOverrides;
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
   * A short line of text: at most `max` characters, no control characters and, when `pattern`
   * is given, matching it. The default is always accepted.
   */
  | { type: "text"; scope: PreferenceScope; default: string; max: number; pattern?: RegExp }
  /** Some of `options`, each at most once, kept in the order of `options`. */
  | {
      type: "set";
      scope: PreferenceScope;
      options: readonly string[];
      default: readonly string[];
    }
  /** Binding ids to the keys they have (see {@link ShortcutOverrides}). */
  | { type: "bindings"; scope: PreferenceScope; default: ShortcutOverrides }
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

/** The longest still duration, in seconds (one hour, the engine's limit for `project.new`). */
export const STILL_DURATION_MAX = 3600;
export const RECENT_COUNT_MAX = 25;
/** The most bindings a person can change, keys per binding, and the length of one key. */
export const MAX_SHORTCUT_OVERRIDES = 512;
export const MAX_KEYS_PER_BINDING = 4;
const MAX_KEY_LENGTH = 64;
const BINDING_ID = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,95}$/;
/** `Mod+Shift+Z`, `F2`, `Mod+K Mod+S`: one stroke, or two with a space between. */
const KEY_SPELLING = /^\S+(?: \S+)?$/u;
const MAX_PATH_LENGTH = 4096;
const MAX_RECORD_ENTRIES = 64;
const RECORD_KEY = /^[a-z0-9][a-z0-9._-]{0,63}$/;
const MAX_RECORD_VALUE = 64;
/** An export preset id (the engine's rule: lowercase letters, digits and `-`). */
const PRESET_ID = /^[a-z0-9][a-z0-9-]*$/;
/** A file name pattern: no folder separator and nothing else a file name cannot hold. */
const FILE_NAME_PATTERN = /^[^\\/:*?"<>|]+$/;

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
  // Export (WP-2G.04): preset ids are the engine's (`export.list_presets`).
  "export.preset": { type: "text", scope: "app", default: "", max: 64, pattern: PRESET_ID },
  "export.last_preset": { type: "text", scope: "app", default: "", max: 64, pattern: PRESET_ID },
  "export.filename": {
    type: "text",
    scope: "app",
    default: "{project}",
    max: 128,
    pattern: FILE_NAME_PATTERN,
  },
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
  "cache.proxies_limit": {
    type: "enum",
    scope: "machine",
    options: PROXY_LIMITS,
    default: 50 * GIB,
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
  "performance.proxy_jobs": {
    type: "enum",
    scope: "machine",
    options: PROXY_JOB_CHOICES,
    default: 2,
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
  // Editing (WP-2F.05b, ADR-0035)
  "editing.still_duration": {
    type: "int",
    scope: "app",
    min: 1,
    max: STILL_DURATION_MAX,
    default: 5,
  },
  // Media and import (WP-1M.14)
  "media.hover_scrub": { type: "boolean", scope: "app", default: true },
  "media.keep_structure": { type: "boolean", scope: "app", default: true },
  "media.copy_on_import": { type: "boolean", scope: "app", default: false },
  "media.image_sequence_fps": {
    type: "enum",
    scope: "app",
    options: IMAGE_SEQUENCE_FPS_CHOICES,
    default: "sequence",
  },
  // Proxies (WP-2F.03b): the policy is the app's and copied into new projects; the size is
  // this computer's.
  "media.proxy_policy": { type: "enum", scope: "app", options: PROXY_POLICIES, default: "auto" },
  "media.proxy_size": { type: "enum", scope: "machine", options: PROXY_SIZES, default: 960 },
  // Editing (WP-2A.13)
  "editing.link_av": { type: "boolean", scope: "app", default: true },
  "editing.drop_mode": { type: "enum", scope: "app", options: DROP_MODES, default: "overwrite" },
  "editing.nudge": { type: "int", scope: "app", min: 1, max: NUDGE_MAX, default: 1 },
  "editing.undo_steps": { type: "int", scope: "app", min: 1, max: UNDO_STEPS_MAX, default: 1000 },
  // Timeline (WP-2A.13)
  "timeline.snap": { type: "boolean", scope: "app", default: true },
  "timeline.snap_targets": {
    type: "set",
    scope: "app",
    options: SNAP_TARGETS,
    default: SNAP_TARGETS,
  },
  "timeline.snap_distance": {
    type: "int",
    scope: "app",
    min: SNAP_DISTANCE_MIN,
    max: SNAP_DISTANCE_MAX,
    default: 8,
  },
  "timeline.follow": { type: "enum", scope: "app", options: FOLLOW_MODES, default: "page" },
  "timeline.magnetic_main": { type: "boolean", scope: "app", default: true },
  "timeline.thumbnails": {
    type: "enum",
    scope: "app",
    options: THUMBNAIL_MODES,
    default: "filmstrip",
  },
  "timeline.waveforms": { type: "boolean", scope: "app", default: true },
  "timeline.track_height": {
    type: "enum",
    scope: "app",
    options: TRACK_HEIGHTS,
    default: "medium",
  },
  "timeline.wheel": { type: "enum", scope: "app", options: WHEEL_MODES, default: "scroll" },
  "timeline.time_display": {
    type: "enum",
    scope: "app",
    options: TIME_DISPLAYS,
    default: "timecode",
  },
  "timeline.drop_frame": { type: "boolean", scope: "app", default: true },
  // Keyboard shortcuts (WP-2H.05)
  "history.order": { type: "enum", scope: "app", options: HISTORY_ORDERS, default: "oldest" },
  "shortcuts.preset": { type: "enum", scope: "app", options: SHORTCUT_PRESETS, default: "nle" },
  "shortcuts.bindings": { type: "bindings", scope: "app", default: {} },
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
    case "set": {
      if (!Array.isArray(value)) return { ok: false, reason: "expected a list of choices" };
      if (value.some((item) => !def.options.includes(item as string))) {
        return { ok: false, reason: `expected some of ${def.options.join(", ")}` };
      }
      if (new Set(value).size !== value.length) {
        return { ok: false, reason: "a choice is listed twice" };
      }
      return { ok: true, value: def.options.filter((option) => value.includes(option)) };
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
    case "text": {
      if (typeof value !== "string") return { ok: false, reason: "expected text" };
      if (value === def.default) return { ok: true, value };
      if (value.length > def.max || /\p{Cc}/u.test(value)) {
        return { ok: false, reason: `expected at most ${def.max} characters on one line` };
      }
      return def.pattern === undefined || def.pattern.test(value)
        ? { ok: true, value }
        : { ok: false, reason: "has characters this setting cannot hold" };
    }
    case "bindings": {
      if (!isPlainObject(value)) return { ok: false, reason: "expected a list of shortcuts" };
      const entries = Object.entries(value);
      if (entries.length > MAX_SHORTCUT_OVERRIDES) {
        return { ok: false, reason: "too many shortcuts" };
      }
      const clean: ShortcutOverrides = {};
      for (const [id, keys] of entries) {
        if (!BINDING_ID.test(id)) return { ok: false, reason: `"${id}" is not a command` };
        if (!Array.isArray(keys) || keys.length > MAX_KEYS_PER_BINDING) {
          return { ok: false, reason: `"${id}" has too many keys` };
        }
        const spelled: string[] = [];
        for (const key of keys) {
          if (typeof key !== "string" || key.length > MAX_KEY_LENGTH || !KEY_SPELLING.test(key)) {
            return { ok: false, reason: `"${id}" has a key that is not valid` };
          }
          if (!spelled.includes(key)) spelled.push(key);
        }
        clean[id] = spelled;
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
      def.type === "record" || def.type === "bindings"
        ? structuredClone(def.default)
        : def.type === "paths" || def.type === "set"
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
      ka.length === Object.keys(b).length &&
      ka.every((k) => Object.hasOwn(b, k) && samePreferenceValue(a[k], b[k]))
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
  /** Only the keys that start with this (`"shortcuts."`: "Export shortcuts…"). */
  only?: string,
): { text: string; count: number } {
  const app: Record<string, unknown> = {};
  for (const key of PREFERENCE_KEYS) {
    if (PREFERENCES[key].scope !== "app") continue;
    if (only !== undefined && !key.startsWith(only)) continue;
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
export function parseImport(text: string, only?: string): ParsedImport {
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
    if (only !== undefined && !key.startsWith(only)) {
      skipped.push({ key, reason: "not a shortcut" });
    } else if (!isPreferenceKey(key)) {
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
  if (only !== undefined) {
    // A shortcuts file replaces the shortcuts: what it leaves out is back to its default.
    const defaults = defaultPreferences();
    for (const key of PREFERENCE_KEYS) {
      if (key.startsWith(only) && !Object.hasOwn(applied, key)) applied[key] = defaults[key];
    }
  }
  return { ok: true, applied: applied as PreferenceChanges, skipped };
}
