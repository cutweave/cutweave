// SPDX-License-Identifier: GPL-3.0-or-later
import type { FrameRate, MediaId, PreviewQuality } from "@cutweave/protocol";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

export type PlaybackMode = "stopped" | "playing" | "scrubbing";

/**
 * A media item the engine plays on its own instead of the timeline (`playback.state.source`,
 * `view.set_source`, ADR-0022).
 */
export interface PlaybackSource {
  media: MediaId;
  /** Its length in flicks: the source's own time runs from 0 to here. */
  duration: number;
  /** Its frame grid (the asset's own rate; 30 fps for audio-only media). */
  fps: FrameRate;
}

/**
 * Whether the engine runs the transport (`view.play`, `view.seek`, ...): `unknown` until the
 * first transport command answers, `unsupported` after one came back `not_found` (an engine
 * without the playback subsystem, WP-1I.05).
 */
export type TransportSupport = "unknown" | "available" | "unsupported";

/**
 * Transport state as the engine reports it (`playback.state`, at most 30 times a second, and the
 * result of every `view.*` transport command). The payload type is not generated yet, so
 * {@link parsePlaybackState} accepts what it can read and ignores the rest.
 */
export interface PlaybackState {
  mode: PlaybackMode;
  /** Whether the playhead advances. */
  playing: boolean;
  /** Playback rate: 1 = normal, 2 and 4 fast forward, negative = reverse, 0 when stopped. */
  rate: number;
  /**
   * The timeline's live playhead in flicks (the audio clock while playing at 1x). It does not
   * move while a `source` plays.
   */
  playhead: number;
  /**
   * The media item playing instead of the timeline, or `null`. While it is set, `mode`,
   * `playing` and `rate` describe the source, and its playhead is `sourcePlayhead`.
   */
  source: PlaybackSource | null;
  /** The source's playhead in flicks, in its own time (meaningful while `source` is set). */
  sourcePlayhead: number;
  /** Resolution of the newest delivered frame. */
  quality: PreviewQuality;
  /** Frames the engine skipped because rendering fell behind (since playback attached). */
  dropped: number;
  /** `performance.now()` when `playhead` was last set: extrapolate from here while playing. */
  sampledAt: number;
  transport: TransportSupport;
  /**
   * While a seek the user made is on its way, events still report the old position: the
   * playhead holds what the user chose until the transport releases it.
   */
  holdPlayhead: boolean;
}

export interface PlaybackActions {
  /** Takes a `playback.state` payload (unknown shape: unusable fields are ignored). */
  applyState(payload: unknown): void;
  /** Moves the playhead ahead of the engine's answer (a seek the user just made). */
  setPlayhead(flicks: number): void;
  setTransport(transport: TransportSupport): void;
  setHoldPlayhead(hold: boolean): void;
}

export type PlaybackStore = PlaybackState & PlaybackActions;

export const initialPlaybackState: PlaybackState = {
  mode: "stopped",
  playing: false,
  rate: 1,
  playhead: 0,
  source: null,
  sourcePlayhead: 0,
  quality: "full",
  dropped: 0,
  sampledAt: 0,
  transport: "unknown",
  holdPlayhead: false,
};

const MODES: readonly string[] = ["stopped", "playing", "scrubbing"];
const QUALITIES: readonly string[] = ["full", "half", "quarter"];

type Parsed = Partial<
  Omit<PlaybackState, "sampledAt" | "transport" | "holdPlayhead" | "sourcePlayhead">
>;

const isCount = (v: unknown): v is number => typeof v === "number" && Number.isInteger(v) && v >= 0;

/** `playback.state.source`: `null`, a source, or `undefined` when unusable. */
function parseSource(value: unknown): PlaybackSource | null | undefined {
  if (value === null) return null;
  if (typeof value !== "object") return undefined;
  const v = value as Record<string, unknown>;
  const fps = v.fps as Record<string, unknown> | null | undefined;
  if (
    typeof v.media !== "string" ||
    !isCount(v.duration) ||
    typeof fps !== "object" ||
    fps === null ||
    !isCount(fps.num) ||
    !isCount(fps.den) ||
    fps.num === 0 ||
    fps.den === 0
  ) {
    return undefined;
  }
  return {
    media: v.media as MediaId,
    duration: v.duration,
    fps: { num: fps.num, den: fps.den },
  };
}

/**
 * Reads the fields of a `playback.state` payload it understands: `state` (or `mode`),
 * `playing`, `rate` (or `rate_milli`, 1000 = 1x), `t` (or `playhead` / `pts`, flicks),
 * `quality`, `dropped` and `source` (ADR-0022). `playhead` is the time of what plays: the
 * timeline's, or the source's when `source` is set.
 */
export function parsePlaybackState(payload: unknown): Parsed {
  if (typeof payload !== "object" || payload === null) {
    return {};
  }
  const p = payload as Record<string, unknown>;
  const out: Parsed = {};
  const mode = p.state ?? p.mode;
  if (typeof mode === "string" && MODES.includes(mode)) out.mode = mode as PlaybackMode;
  if (typeof p.playing === "boolean") out.playing = p.playing;
  else if (out.mode !== undefined) out.playing = out.mode === "playing";
  if (typeof p.rate === "number" && Number.isFinite(p.rate)) out.rate = p.rate;
  else if (typeof p.rate_milli === "number" && Number.isFinite(p.rate_milli)) {
    out.rate = p.rate_milli / 1000;
  }
  const playhead = p.t ?? p.playhead ?? p.pts;
  if (typeof playhead === "number" && Number.isFinite(playhead) && playhead >= 0) {
    out.playhead = playhead;
  }
  if (typeof p.quality === "string" && QUALITIES.includes(p.quality)) {
    out.quality = p.quality as PreviewQuality;
  }
  if (isCount(p.dropped)) {
    out.dropped = p.dropped;
  }
  if ("source" in p) {
    const source = parseSource(p.source);
    if (source !== undefined) out.source = source;
  }
  return out;
}

/** `now` stamps each playhead sample (the page's monotonic clock; tests pass a fake). */
export function createPlaybackStore(now: () => number = () => performance.now()) {
  return createStore<PlaybackStore>()((set, get) => ({
    ...initialPlaybackState,
    applyState: (payload) => {
      const { playhead, ...rest } = parsePlaybackState(payload);
      // A payload without `source` describes what already plays.
      const source = rest.source !== undefined ? rest.source : get().source;
      let next: Partial<PlaybackState> = rest;
      if (playhead !== undefined && source !== null) {
        // A source's time never moves the timeline's playhead.
        next = { ...rest, sourcePlayhead: playhead };
      } else if (playhead !== undefined && !get().holdPlayhead) {
        next = { ...rest, playhead, sampledAt: now() };
      }
      if (Object.keys(next).length === 0) return;
      set(next);
    },
    setPlayhead: (flicks) => {
      if (!Number.isFinite(flicks)) return;
      set({ playhead: Math.max(0, Math.round(flicks)), sampledAt: now() });
    },
    setTransport: (transport) => set({ transport }),
    setHoldPlayhead: (holdPlayhead) => set({ holdPlayhead }),
  }));
}

export const playbackStore = createPlaybackStore();

export function usePlaybackStore<T>(selector: (state: PlaybackStore) => T): T {
  return useStore(playbackStore, selector);
}
