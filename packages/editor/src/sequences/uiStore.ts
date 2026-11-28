// SPDX-License-Identifier: GPL-3.0-or-later
// Session-only state of the sequence UI (WP-2B.03): the tab being renamed and the sequence whose
// settings dialog is open. Nothing here is saved or sent to the engine.
import type { SequenceId } from "@cutweave/protocol";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

export interface SequenceUiState {
  /** The tab whose name is being edited in place. */
  readonly renaming: SequenceId | null;
  /** The sequence Sequence settings shows; `null`: the dialog is closed. */
  readonly settings: SequenceId | null;
  startRename(sequence: SequenceId): void;
  endRename(): void;
  openSettings(sequence: SequenceId): void;
  closeSettings(): void;
}

export const sequenceUiStore = createStore<SequenceUiState>()((set) => ({
  renaming: null,
  settings: null,
  startRename: (renaming) => set({ renaming }),
  endRename: () => set({ renaming: null }),
  openSettings: (settings) => set({ settings }),
  closeSettings: () => set({ settings: null }),
}));

export function useSequenceUi<T>(selector: (state: SequenceUiState) => T): T {
  return useStore(sequenceUiStore, selector);
}
