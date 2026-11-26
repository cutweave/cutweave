// SPDX-License-Identifier: GPL-3.0-or-later
// What the export dialog, the top bar's export chip and the queue panel share (WP-1M.08,
// WP-2G.04): whether the dialog is open, the exports this window started and follows, the
// presets and encoders the engine offers, and what the last start said. The jobs' own progress
// is `jobStore` (the engine's `job.progress` events); this store only remembers which jobs are
// ours.

import type { CodecEncoders, ExportPresetEntry, JobId, PresetWarning } from "@cutweave/protocol";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import { isActive, type JobStore, jobStore } from "../state/jobStore.ts";

/** One file of the export this window started and is following. */
export interface ActiveFile {
  readonly jobId: JobId;
  /** Where the file goes (what `export.start` answered). */
  readonly path: string;
  /** Its file name, for messages. */
  readonly fileName: string;
}

/** The export this window started with Export (one file, or every file of a marker set). */
export interface ActiveExport {
  readonly files: readonly ActiveFile[];
  /** The jobs have been seen in `jobStore`: if one vanishes after that, the engine restarted. */
  readonly seen: boolean;
  /** What the engine said when it started (fit, limits, software encoder). */
  readonly warnings: readonly PresetWarning[];
  /** The engine's estimate of the files' size in KiB, when the rate control fixes it. */
  readonly estimatedKib: number | null;
}

/** A list the dialog asks the engine for once it opens. */
export type Loaded<T> =
  | { readonly status: "idle" | "loading" | "unavailable" }
  | { readonly status: "ready"; readonly value: T };

export interface ExportState {
  readonly dialogOpen: boolean;
  readonly active: ActiveExport | null;
  /** Cancel was pressed; the jobs end when the engine next checks. */
  readonly cancelling: boolean;
  /** `export.list_presets`. */
  readonly presets: Loaded<readonly ExportPresetEntry[]>;
  /** `export.list_encoders`: the software and verified hardware encoders per codec. */
  readonly encoders: Loaded<readonly CodecEncoders[]>;
}

export interface ExportStore extends ExportState {
  setDialogOpen(open: boolean): void;
  setActive(active: ActiveExport | null): void;
  markSeen(): void;
  setCancelling(cancelling: boolean): void;
  setPresets(presets: ExportState["presets"]): void;
  setEncoders(encoders: ExportState["encoders"]): void;
}

export const initialExportState: ExportState = {
  dialogOpen: false,
  active: null,
  cancelling: false,
  presets: { status: "idle" },
  encoders: { status: "idle" },
};

export function createExportStore() {
  return createStore<ExportStore>()((set) => ({
    ...initialExportState,
    setDialogOpen: (dialogOpen) => set({ dialogOpen }),
    setActive: (active) => set({ active, cancelling: false }),
    markSeen: () => set((s) => (s.active === null ? s : { active: { ...s.active, seen: true } })),
    setCancelling: (cancelling) => set({ cancelling }),
    setPresets: (presets) => set({ presets }),
    setEncoders: (encoders) => set({ encoders }),
  }));
}

export const exportStore = createExportStore();

export function useExportStore<T>(selector: (state: ExportStore) => T): T {
  return useStore(exportStore, selector);
}

/**
 * The job of the active export that is still unfinished (the running file of a marker set),
 * else its first; `undefined` with no active export.
 */
export function runningExportJob(
  state: Pick<ExportState, "active"> = exportStore.getState(),
  jobs: JobStore = jobStore.getState(),
): JobId | undefined {
  const files = state.active?.files ?? [];
  const open = files.find((f) => {
    const info = jobs.jobs[f.jobId];
    return info === undefined || isActive(info);
  });
  return (open ?? files[0])?.jobId;
}
