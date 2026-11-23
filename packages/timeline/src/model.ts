// SPDX-License-Identifier: GPL-3.0-or-later
// The read-only timeline projection the renderer consumes (arch §16: `docStore` is a normalized
// read-only projection of the engine doc). No engine dependency: fixtures build these directly.
// All times are flicks (`number`, integer). Entities are addressed by id, never by index.
import type { FrameRate } from "./time.ts";

export type ItemKind = "media" | "audio" | "graphics" | "captions" | "effects";
export type TrackKind = ItemKind;
/** Lane height class: the `track-sm` / `track-md` / `track-lg` tokens. */
export type TrackSize = "sm" | "md" | "lg";

export interface TimelineTrack {
  readonly id: string;
  readonly name: string;
  readonly kind: TrackKind;
  readonly size?: TrackSize;
  readonly muted?: boolean;
  readonly locked?: boolean;
  readonly hidden?: boolean;
  /** Magnetic main track: 2px marigold edge on its header. */
  readonly main?: boolean;
}

/** A slice of a media asset an item shows (thumbnails, waveform), addressed by media id. */
export interface MediaSlice {
  readonly mediaId: string;
  /** Source time (flicks) at the item's left edge. */
  readonly sourceIn: number;
}

export interface ItemState {
  readonly selected?: boolean;
  readonly disabled?: boolean;
  /** A ghost clip proposed by an agent (open proposal transaction). */
  readonly proposed?: boolean;
}

/** Where an item reads its media: the trim limits of WP-1M.06 (a clip never reads outside it). */
export interface SourceRange {
  /** Media position (flicks) at the item's left edge. */
  readonly sourceIn: number;
  /** Media length, flicks; `null` when unbounded (a still, or a length the probe did not report). */
  readonly mediaDuration: number | null;
  /** A still image: it has no source position, so its start edge is not bounded by the media. */
  readonly still?: boolean;
}

export interface TimelineItem {
  readonly id: string;
  readonly trackId: string;
  readonly kind: ItemKind;
  /** Start on the timeline, flicks. */
  readonly start: number;
  /** Duration, flicks (>= 0). */
  readonly duration: number;
  readonly label: string;
  /** Linked group (a video and its audio): members follow each other's edits, never snap to each other. */
  readonly linkedGroup?: string;
  /** Media bounds for trims. Omitted: only the timeline start and the neighbours bound a trim. */
  readonly source?: SourceRange;
  /** Filmstrip tiles come from `mediaId` through the pluggable loader. */
  readonly thumbnails?: MediaSlice;
  /** Waveform peaks come from `mediaId` through the pluggable loader. */
  readonly waveform?: MediaSlice;
  /** Keyframe times, absolute timeline flicks, sorted ascending. */
  readonly keyframes?: readonly number[];
  readonly state?: ItemState;
}

export interface TimelineModel {
  readonly fps: FrameRate;
  readonly dropFrame?: boolean;
  readonly tracks: readonly TimelineTrack[];
  readonly items: readonly TimelineItem[];
}

export const EMPTY_MODEL: TimelineModel = {
  fps: { num: 24, den: 1 },
  tracks: [],
  items: [],
};

export interface Rect {
  readonly x: number;
  readonly y: number;
  readonly w: number;
  readonly h: number;
}

/** View state owned by the host (`viewStore` / `playbackStore`); the renderer only reads it. */
export interface ViewState {
  /** Zoom, CSS pixels per second. */
  readonly pxPerSec: number;
  /** Time (flicks) at the left edge of the lane area. */
  readonly scrollX: number;
  /** Vertical scroll of the track area, CSS pixels. */
  readonly scrollY: number;
  /** Playhead time, flicks. */
  readonly playhead: number;
  /** Selected item ids (in addition to `item.state.selected`). */
  readonly selection: ReadonlySet<string>;
  /** Snap guide times (flicks), drawn as vertical lines. */
  readonly snapGuides: readonly number[];
  /** In/out range shading, flicks. */
  readonly range: { readonly in: number; readonly out: number } | null;
  /** Marquee rectangle in canvas CSS pixels. */
  readonly marquee: Rect | null;
}

export const DEFAULT_VIEW_STATE: ViewState = {
  pxPerSec: 100,
  scrollX: 0,
  scrollY: 0,
  playhead: 0,
  selection: new Set(),
  snapGuides: [],
  range: null,
  marquee: null,
};
