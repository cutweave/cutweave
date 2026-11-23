// SPDX-License-Identifier: GPL-3.0-or-later
import type { PreviewQuality } from "@cutweave/protocol";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

export type PlaybackMode = "stopped" | "playing" | "scrubbing";

/**
 * Transport state as the engine reports it (`playback.state`, WP-1I.05). The payload type is not
 * generated yet, so {@link parsePlaybackState} accepts what it can read and ignores the rest.
 */
export interface PlaybackState {
  mode: PlaybackMode;
  /** Playback rate: 1 = normal, negative = reverse. */
  rate: number;
  /** The live playhead in flicks (the audio clock while playing). */
  playhead: number;
  quality: PreviewQuality;
}

export interface PlaybackActions {
  /** Takes a `playback.state` payload (unknown shape: unusable fields are ignored). */
  applyState(payload: unknown): void;
}

export type PlaybackStore = PlaybackState & PlaybackActions;

export const initialPlaybackState: PlaybackState = {
  mode: "stopped",
  rate: 1,
  playhead: 0,
  quality: "full",
};

const MODES: readonly string[] = ["stopped", "playing", "scrubbing"];
const QUALITIES: readonly string[] = ["full", "half", "quarter"];

/**
 * Reads the fields of a `playback.state` payload it understands: `state` or `mode`, `rate` (or
 * `rate_milli`, 1000 = 1x), `playhead` (or `pts`, flicks) and `quality`.
 */
export function parsePlaybackState(payload: unknown): Partial<PlaybackState> {
  if (typeof payload !== "object" || payload === null) {
    return {};
  }
  const p = payload as Record<string, unknown>;
  const out: Partial<PlaybackState> = {};
  const mode = p.state ?? p.mode;
  if (typeof mode === "string" && MODES.includes(mode)) out.mode = mode as PlaybackMode;
  if (typeof p.rate === "number" && Number.isFinite(p.rate)) out.rate = p.rate;
  else if (typeof p.rate_milli === "number" && Number.isFinite(p.rate_milli)) {
    out.rate = p.rate_milli / 1000;
  }
  const playhead = p.playhead ?? p.pts;
  if (typeof playhead === "number" && Number.isFinite(playhead) && playhead >= 0) {
    out.playhead = playhead;
  }
  if (typeof p.quality === "string" && QUALITIES.includes(p.quality)) {
    out.quality = p.quality as PreviewQuality;
  }
  return out;
}

export function createPlaybackStore() {
  return createStore<PlaybackStore>()((set) => ({
    ...initialPlaybackState,
    applyState: (payload) => {
      const next = parsePlaybackState(payload);
      if (Object.keys(next).length > 0) set(next);
    },
  }));
}

export const playbackStore = createPlaybackStore();

export function usePlaybackStore<T>(selector: (state: PlaybackStore) => T): T {
  return useStore(playbackStore, selector);
}
