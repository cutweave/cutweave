// SPDX-License-Identifier: GPL-3.0-or-later
// Where the engine's source is shown while one plays (WP-1M.15, ADR-0022). The engine has one
// source and one frame ring; this says which picture draws it: the Preview panel's Source view,
// or the Inspector's own picture. Starting a source from the bin always moves it to the Preview.
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import { playbackStore, usePlaybackStore } from "./playbackStore.ts";

export type SourceHost = "preview" | "inspector";

export interface SourceViewStore {
  host: SourceHost;
  setHost(host: SourceHost): void;
}

export const sourceViewStore = createStore<SourceViewStore>()((set) => ({
  host: "preview",
  setHost: (host) => set({ host }),
}));

export function useSourceViewStore<T>(selector: (state: SourceViewStore) => T): T {
  return useStore(sourceViewStore, selector);
}

/** Whether the Preview panel is in its Source view (a source plays and the Preview hosts it). */
export function usePreviewShowsSource(): boolean {
  const source = usePlaybackStore((s) => s.source !== null);
  const host = useSourceViewStore((s) => s.host);
  return source && host === "preview";
}

/** Whether the Preview's picture is held while the Inspector draws the source. */
export function usePreviewFrozen(): boolean {
  const source = usePlaybackStore((s) => s.source !== null);
  const host = useSourceViewStore((s) => s.host);
  return source && host === "inspector";
}

// A source that ends (a removed asset, another project) returns the Preview to the timeline.
playbackStore.subscribe((next, prev) => {
  if (next.source === null && prev.source !== null) sourceViewStore.getState().setHost("preview");
});
