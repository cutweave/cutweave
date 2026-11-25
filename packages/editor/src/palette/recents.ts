// SPDX-License-Identifier: GPL-3.0-or-later
// The commands the person ran most recently, newest first (WP-2H.04: "recent commands first").
// A per-viewer convenience kept in `localStorage`; it comes back empty when storage is blocked.
import { createStore } from "zustand/vanilla";

export const RECENTS_KEY = "cutweave.palette.recent";
export const MAX_RECENTS = 8;

function read(): string[] {
  try {
    const parsed: unknown = JSON.parse(globalThis.localStorage?.getItem(RECENTS_KEY) ?? "[]");
    return Array.isArray(parsed)
      ? parsed.filter((k): k is string => typeof k === "string").slice(0, MAX_RECENTS)
      : [];
  } catch {
    return [];
  }
}

function write(keys: readonly string[]): void {
  try {
    globalThis.localStorage?.setItem(RECENTS_KEY, JSON.stringify(keys));
  } catch {
    // Blocked storage: the list lives for this session only.
  }
}

interface RecentsState {
  /** Entry keys (`command` or `command#args`), newest first. */
  readonly keys: readonly string[];
  push(key: string): void;
  clear(): void;
}

export const recentsStore = createStore<RecentsState>()((set) => ({
  keys: read(),
  push: (key) =>
    set((s) => {
      const keys = [key, ...s.keys.filter((k) => k !== key)].slice(0, MAX_RECENTS);
      write(keys);
      return { keys };
    }),
  clear: () => {
    write([]);
    set({ keys: [] });
  },
}));
