// SPDX-License-Identifier: GPL-3.0-or-later
// What the History panel shows (WP-2H.11): the engine's history entries, kept current by
// `HistoryFeed`, and the one message a refused jump leaves. Read-only projection; every change
// of the project goes through `edit.undo` / `edit.redo`.

import type { HistoryEntry } from "@cutweave/protocol";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

export interface HistoryState {
  /** The engine's entries, newest first (undo and redo records included). */
  readonly entries: readonly HistoryEntry[];
  /** The first page arrived. */
  readonly loaded: boolean;
  /** A jump is running: the rows do not take another until it finishes. */
  readonly busy: boolean;
  /** Why the last jump stopped (the engine's message naming the later change), or `null`. */
  readonly notice: string | null;
}

export interface HistoryStore extends HistoryState {
  setEntries(entries: readonly HistoryEntry[]): void;
  setBusy(busy: boolean): void;
  setNotice(notice: string | null): void;
  reset(): void;
}

export const initialHistoryState: HistoryState = {
  entries: [],
  loaded: false,
  busy: false,
  notice: null,
};

export function createHistoryStore() {
  return createStore<HistoryStore>()((set) => ({
    ...initialHistoryState,
    setEntries: (entries) => set({ entries, loaded: true }),
    setBusy: (busy) => set({ busy }),
    setNotice: (notice) => set({ notice }),
    reset: () => set({ ...initialHistoryState }),
  }));
}

export const historyStore = createHistoryStore();

export function useHistoryStore<T>(selector: (state: HistoryStore) => T): T {
  return useStore(historyStore, selector);
}
