// SPDX-License-Identifier: GPL-3.0-or-later
// The preferences store (WP-1M.11, arch §16): `<userData>/preferences.json`, next to
// `recent-projects.json` and `window-state.json`. Main owns the file; the editor reads a snapshot
// and writes one validated key at a time through IPC (src/main/ipc.ts). The schema (keys, types,
// defaults, migrations) is the one the editor imports, `packages/preferences`.
//
// - A damaged file, an unknown key, a wrong type or an out-of-range value never blocks startup:
//   each falls back to its default, a warning goes to the main log and the file is rewritten clean.
// - Writes are atomic: a temp file in the same folder is written and fsynced, then renamed over
//   the old one, so a kill at any moment leaves either the old file or the new one.
// - Secrets never go here (arch §13), and neither do recent projects or window state: they have
//   their own files, which "Reset all settings" never touches.

import {
  closeSync,
  fsyncSync,
  mkdirSync,
  openSync,
  readdirSync,
  readFileSync,
  renameSync,
  unlinkSync,
  writeSync,
} from "node:fs";
import path from "node:path";
import {
  defaultPreferences,
  exportPreferences,
  loadPreferences,
  type PreferenceChanges,
  type PreferenceKey,
  type PreferenceMap,
  parseImport,
  type SkippedSetting,
  samePreferenceValue,
  serializePreferences,
  validatePreference,
} from "../../../../packages/preferences/src/index.ts";

/** The file operations the store needs; tests replace them to cut a write short. */
export interface PreferencesFiles {
  /** The file's text, or null when it does not exist or cannot be read. */
  read(file: string): string | null;
  /** Writes `text` so that `file` holds either its old content or all of the new. */
  writeAtomic(file: string, text: string): void;
  /** Removes temp files an interrupted write left beside `file`. */
  sweep(file: string): void;
}

/** The pieces of `node:fs` an atomic write uses (tests override `renameSync`). */
export interface AtomicOps {
  renameSync: typeof renameSync;
}

export function nodeFiles(ops: AtomicOps = { renameSync }): PreferencesFiles {
  return {
    read(file) {
      try {
        return readFileSync(file, "utf8");
      } catch {
        return null;
      }
    },
    writeAtomic(file, text) {
      mkdirSync(path.dirname(file), { recursive: true });
      const temp = `${file}.${process.pid}.tmp`;
      const fd = openSync(temp, "w");
      try {
        writeSync(fd, text);
        fsyncSync(fd);
      } finally {
        closeSync(fd);
      }
      try {
        ops.renameSync(temp, file);
      } catch (error) {
        try {
          unlinkSync(temp);
        } catch {
          // The temp file is already gone.
        }
        throw error;
      }
    },
    sweep(file) {
      try {
        const dir = path.dirname(file);
        const prefix = `${path.basename(file)}.`;
        for (const name of readdirSync(dir)) {
          if (name.startsWith(prefix) && name.endsWith(".tmp")) unlinkSync(path.join(dir, name));
        }
      } catch {
        // Nothing to clean, or the folder cannot be read.
      }
    },
  };
}

export class PreferenceError extends Error {
  constructor(detail: string) {
    super(`invalid preference: ${detail}`);
    this.name = "PreferenceError";
  }
}

export interface PreferencesOptions {
  files?: PreferencesFiles;
  /** Defaults that depend on this machine (the Documents folder), applied under the file. */
  contextDefaults?: PreferenceChanges;
  /** Receives the warnings of a damaged or partly invalid file. */
  log?: (message: string) => void;
}

export type ImportOutcome =
  | { status: "imported"; applied: PreferenceKey[]; skipped: SkippedSetting[] }
  | { status: "invalid"; reason: "unreadable" | "not-settings" | "newer" };

type Listener = (changes: PreferenceChanges) => void;

export class PreferencesStore {
  readonly #file: string;
  readonly #files: PreferencesFiles;
  readonly #log: (message: string) => void;
  readonly #defaults: PreferenceMap;
  #values: PreferenceMap;
  readonly #listeners = new Set<Listener>();

  constructor(file: string, options: PreferencesOptions = {}) {
    this.#file = file;
    this.#files = options.files ?? nodeFiles();
    this.#log = options.log ?? (() => undefined);
    this.#defaults = defaultPreferences(options.contextDefaults);
    this.#files.sweep(file);
    const loaded = loadPreferences(this.#files.read(file), options.contextDefaults);
    for (const warning of loaded.warnings) this.#log(`preferences: ${warning}`);
    this.#values = loaded.values;
    if (loaded.rewrite) this.#persist();
  }

  /** Every key's current value (a copy: nothing outside can change the store). */
  snapshot(): PreferenceMap {
    return structuredClone(this.#values);
  }

  /** Every key's default on this machine. */
  defaults(): PreferenceMap {
    return structuredClone(this.#defaults);
  }

  get<K extends PreferenceKey>(key: K): PreferenceMap[K] {
    return structuredClone(this.#values[key]);
  }

  /** Validates and stores one key; throws `PreferenceError` for an unknown key or a bad value. */
  set(key: string, value: unknown): void {
    const checked = validatePreference(key, value);
    if (!checked.ok) throw new PreferenceError(`${key}: ${checked.reason}`);
    this.#apply({ [key]: checked.value } as PreferenceChanges);
  }

  /** Puts these keys back to their defaults. */
  reset(keys: readonly PreferenceKey[]): void {
    const changes: Record<string, unknown> = {};
    for (const key of keys) changes[key] = structuredClone(this.#defaults[key]);
    this.#apply(changes as PreferenceChanges);
  }

  /** The text of "Export settings…": app values that differ from their defaults. */
  exportText(): { text: string; count: number } {
    return exportPreferences(this.#values, this.#defaults);
  }

  /** "Import settings…": applies the valid keys and reports the rest. */
  importText(text: string): ImportOutcome {
    const parsed = parseImport(text);
    if (!parsed.ok) return { status: "invalid", reason: parsed.reason };
    this.#apply(parsed.applied);
    return {
      status: "imported",
      applied: Object.keys(parsed.applied) as PreferenceKey[],
      skipped: parsed.skipped,
    };
  }

  /** Called with the keys that changed, after they are stored. */
  onChange(listener: Listener): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  #apply(changes: PreferenceChanges): void {
    const changed: Record<string, unknown> = {};
    for (const [key, value] of Object.entries(changes)) {
      const k = key as PreferenceKey;
      if (samePreferenceValue(this.#values[k], value)) continue;
      (this.#values as Record<string, unknown>)[k] = value;
      changed[k] = structuredClone(value);
    }
    if (Object.keys(changed).length === 0) return;
    this.#persist();
    for (const listener of this.#listeners) listener(changed as PreferenceChanges);
  }

  #persist(): void {
    try {
      this.#files.writeAtomic(this.#file, serializePreferences(this.#values, this.#defaults));
    } catch (error) {
      // The value stays in effect for this run; say so, and try again on the next change.
      this.#log(
        `preferences: could not write ${this.#file}: ${error instanceof Error ? error.message : String(error)}`,
      );
    }
  }
}
