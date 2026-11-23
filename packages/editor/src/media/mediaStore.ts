// SPDX-License-Identifier: GPL-3.0-or-later
// What the engine reports about assets outside the document (arch §7 "Jobs and analysis"):
// analysis status is not document state. `media.analysis_failed` marks an asset (it stays
// usable, a stage such as thumbnails is missing) and `media.indexed` says its frame index is
// ready, which is when thumbnails can be fetched (WP-1I.03). Fed by `startEngineRuntime`.
import type { MediaAnalysisFailed, MediaId } from "@cutweave/protocol";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

export interface AnalysisFailure {
  /** The failed stage: `index`, `thumbnails`, `waveform`. */
  readonly stage: string;
  /** What went wrong, as the engine says it. */
  readonly error: string;
}

export interface MediaState {
  /** Failed analysis stages per asset (the latest failure of each stage). */
  readonly failures: Readonly<Record<string, readonly AnalysisFailure[]>>;
  /** How often each asset has been (re)indexed: a changing value tells thumbnails to reload. */
  readonly indexed: Readonly<Record<string, number>>;
}

export interface MediaActions {
  /** A `media.analysis_failed` event. */
  applyAnalysisFailed(event: MediaAnalysisFailed): void;
  /** A `media.indexed` event. */
  applyIndexed(event: { readonly media_id: MediaId }): void;
  /** Forgets everything (a project was opened or closed). */
  reset(): void;
}

export type MediaStore = MediaState & MediaActions;

export function createMediaStore() {
  return createStore<MediaStore>()((set) => ({
    failures: {},
    indexed: {},
    applyAnalysisFailed: (event) =>
      set((s) => {
        const others = (s.failures[event.media_id] ?? []).filter((f) => f.stage !== event.stage);
        return {
          failures: {
            ...s.failures,
            [event.media_id]: [...others, { stage: event.stage, error: event.error }],
          },
        };
      }),
    applyIndexed: (event) =>
      set((s) => {
        const failures = { ...s.failures };
        delete failures[event.media_id];
        return {
          failures,
          indexed: { ...s.indexed, [event.media_id]: (s.indexed[event.media_id] ?? 0) + 1 },
        };
      }),
    reset: () => set({ failures: {}, indexed: {} }),
  }));
}

export const mediaStore = createMediaStore();

export function useMediaStore<T>(selector: (state: MediaStore) => T): T {
  return useStore(mediaStore, selector);
}
