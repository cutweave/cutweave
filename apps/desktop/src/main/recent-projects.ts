// SPDX-License-Identifier: GPL-3.0-or-later
// The recent-projects list (WP-1M.07): the project documents or folders the user opened or saved,
// newest first, kept in the profile folder. The renderer asks for it through validated IPC; main
// never opens a path from it by itself, and the list is only a menu: opening a recent entry goes
// through `project.open` like any other open. Entries are absolute paths; anything else in the
// file is ignored.

import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

/** The list keeps this many entries. */
export const MAX_RECENT = 10;

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
  #items: string[];

  constructor(file: string, files: RecentFiles = nodeFiles) {
    this.#file = file;
    this.#files = files;
    this.#items = this.#load();
  }

  #load(): string[] {
    const text = this.#files.read(this.#file);
    if (text === null) return [];
    try {
      const parsed: unknown = JSON.parse(text);
      if (!Array.isArray(parsed)) return [];
      return [...new Set(parsed.filter(isRecentPath))].slice(0, MAX_RECENT);
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
    return [...this.#items];
  }

  /** Puts `entry` first (moving it up when it is already there). */
  add(entry: string): void {
    if (!isRecentPath(entry)) return;
    this.#items = [entry, ...this.#items.filter((p) => p !== entry)].slice(0, MAX_RECENT);
    this.#save();
  }

  remove(entry: string): void {
    this.#items = this.#items.filter((p) => p !== entry);
    this.#save();
  }
}
