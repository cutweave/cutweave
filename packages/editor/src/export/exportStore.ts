// SPDX-License-Identifier: GPL-3.0-or-later
// What the export dialog and the top bar's export chip share (WP-1M.08): whether the dialog is
// open, the export the engine is running, and the hardware encoders this computer has. The job's
// own progress is `jobStore` (the engine's `job.progress` events); this store only remembers
// which job is ours.

import type { JobId } from "@cutweave/protocol";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

/** The export this window started and is following. */
export interface ActiveExport {
  readonly jobId: JobId;
  /** Where the file goes (what `export.start` answered). */
  readonly path: string;
  /** Its file name, for messages. */
  readonly fileName: string;
  /** The job has been seen in `jobStore`: if it vanishes after that, the engine restarted. */
  readonly seen: boolean;
}

/** The hardware encoders `media.capabilities` lists, once asked. */
export type EncoderProbe =
  | { readonly status: "idle" | "loading" | "unavailable" }
  | { readonly status: "ready"; readonly encoders: readonly string[] };

export interface ExportState {
  readonly dialogOpen: boolean;
  readonly active: ActiveExport | null;
  /** Cancel was pressed; the job ends when the engine next checks. */
  readonly cancelling: boolean;
  readonly probe: EncoderProbe;
}

export interface ExportStore extends ExportState {
  setDialogOpen(open: boolean): void;
  setActive(active: ActiveExport | null): void;
  markSeen(): void;
  setCancelling(cancelling: boolean): void;
  setProbe(probe: EncoderProbe): void;
}

export function createExportStore() {
  return createStore<ExportStore>()((set) => ({
    dialogOpen: false,
    active: null,
    cancelling: false,
    probe: { status: "idle" },
    setDialogOpen: (dialogOpen) => set({ dialogOpen }),
    setActive: (active) => set({ active, cancelling: false }),
    markSeen: () => set((s) => (s.active === null ? s : { active: { ...s.active, seen: true } })),
    setCancelling: (cancelling) => set({ cancelling }),
    setProbe: (probe) => set({ probe }),
  }));
}

export const exportStore = createExportStore();

export function useExportStore<T>(selector: (state: ExportStore) => T): T {
  return useStore(exportStore, selector);
}
