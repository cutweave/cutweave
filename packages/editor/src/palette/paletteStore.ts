// SPDX-License-Identifier: GPL-3.0-or-later
// Whether the command palette is showing (WP-2H.04), and a request to run one of its rows from
// outside the list (the row's own context menu). It opens in the window the person is working in
// (a detached panel's window has its own, WP-2H.15).
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import { activeWindow, isMainWindow } from "../shell/windows.ts";

export interface PaletteState {
  readonly open: boolean;
  /** Bumped by every open, so the palette starts fresh each time. */
  readonly openCount: number;
  /** A row (by key) the row menu asked to run; the palette takes it and clears it. */
  readonly pendingRun: string | null;
  /** The window it opened in, when that is not the main window (`null`: the main window). */
  readonly window: Window | null;
  setOpen(open: boolean): void;
  requestRun(key: string): void;
  clearRun(): void;
}

/** The window the person is in, unless it is the main one (outside a browser: none). */
function openingWindow(): Window | null {
  if (typeof globalThis.document === "undefined") return null;
  const win = activeWindow();
  return isMainWindow(win) ? null : win;
}

export const paletteStore = createStore<PaletteState>()((set) => ({
  open: false,
  openCount: 0,
  pendingRun: null,
  window: null,
  setOpen: (open) =>
    set((s) =>
      open
        ? { open, openCount: s.openCount + 1, pendingRun: null, window: openingWindow() }
        : { open },
    ),
  requestRun: (key) => set({ pendingRun: key }),
  clearRun: () => set({ pendingRun: null }),
}));

export const openPalette = (): void => paletteStore.getState().setOpen(true);
export const closePalette = (): void => paletteStore.getState().setOpen(false);

export function usePaletteOpen(): boolean {
  return useStore(paletteStore, (s) => s.open);
}
