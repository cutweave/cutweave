// SPDX-License-Identifier: GPL-3.0-or-later
// Whether File > Settings… is showing, and where it should open: `openSettings("saving", "saving.interval")`
// opens the dialog on that page with that row focused (a deep link: the title bar's save status
// uses it, and the command palette will, WP-2H.04). One dialog at a time.

import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

export interface SettingsTarget {
  /** The page to open; the first page when it is absent or unknown. */
  readonly page?: string | undefined;
  /** The row to bring into view and focus (a row id, which is its preference key). */
  readonly row?: string | undefined;
}

export interface SettingsState {
  readonly open: boolean;
  readonly target: SettingsTarget;
  /** Bumped by every open, so opening again at another row moves there. */
  readonly openCount: number;
  setOpen(open: boolean): void;
  openAt(target: SettingsTarget): void;
}

export const settingsStore = createStore<SettingsState>()((set) => ({
  open: false,
  target: {},
  openCount: 0,
  setOpen: (open) =>
    set((s) => (open ? { open, target: {}, openCount: s.openCount + 1 } : { open })),
  openAt: (target) => set((s) => ({ open: true, target, openCount: s.openCount + 1 })),
}));

export function useSettingsOpen(): boolean {
  return useStore(settingsStore, (s) => s.open);
}

/** Opens the Settings dialog, on `page` and at the row `row` when they are given. */
export function openSettings(page?: string, row?: string): void {
  settingsStore.getState().openAt({ page, row });
}
