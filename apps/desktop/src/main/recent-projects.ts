// SPDX-License-Identifier: GPL-3.0-or-later
// The recent-projects list (WP-1M.07): the project documents or folders the user opened or saved,
// newest first, kept in the profile folder. The renderer asks for it through validated IPC; main
// never opens a path from it by itself, and the list is only a menu: opening a recent entry goes
// through `project.open` like any other open. Entries are absolute paths; anything else in the
// file is ignored.

import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

/** How many entries the list shows unless the person chose another number (`general.recent_count`). */
export const MAX_RECENT = 10;
/** The file keeps this many, so that raising the number again brings older entries back. */
export const MAX_STORED_RECENT = 25;

export interface RecentFiles {
  read(file: string): string | null;
  write(file: string, text: string): void;
}

const nodeFiles: RecentFiles = {
  read(file) {
    try {
      return readFileSync(file, "utf8");
    } catch {
      return null;
    }
  },
  write: (file, text) => writeFileSync(file, text, "utf8"),
};

/** An absolute, bounded, NUL-free path; the only kind the list holds. */
export function isRecentPath(value: unknown): value is string {
  return (
    typeof value === "string" &&
    value.length > 0 &&
    value.length <= 4096 &&
    !value.includes("\0") &&
    path.isAbsolute(value)
  );
}

export class RecentProjects {
  readonly #file: string;
  readonly #files: RecentFiles;
  readonly #limit: () => number;
  #items: string[];

  /** `limit` is read on every call: the Settings > General > Recent projects number. */
  constructor(
    file: string,
    files: RecentFiles = nodeFiles,
    limit: () => number = () => MAX_RECENT,
  ) {
    this.#file = file;
    this.#files = files;
    this.#limit = limit;
    this.#items = this.#load();
  }

  #load(): string[] {
    const text = this.#files.read(this.#file);
    if (text === null) return [];
    try {
      const parsed: unknown = JSON.parse(text);
      if (!Array.isArray(parsed)) return [];
      return [...new Set(parsed.filter(isRecentPath))].slice(0, MAX_STORED_RECENT);
    } catch {
      return [];
    }
  }

  #save(): void {
    try {
      this.#files.write(this.#file, JSON.stringify(this.#items));
    } catch {
      // The list is a convenience: a profile folder that cannot be written is not an error.
    }
  }

  list(): string[] {
    return this.#items.slice(0, Math.max(0, this.#limit()));
  }

  /** Puts `entry` first (moving it up when it is already there). */
  add(entry: string): void {
    if (!isRecentPath(entry)) return;
    this.#items = [entry, ...this.#items.filter((p) => p !== entry)].slice(0, MAX_STORED_RECENT);
    this.#save();
  }

  /** Forgets every entry, the ones beyond the shown count too. */
  clear(): void {
    this.#items = [];
    this.#save();
  }

  remove(entry: string): void {
    this.#items = this.#items.filter((p) => p !== entry);
    this.#save();
  }
}
