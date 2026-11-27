// SPDX-License-Identifier: GPL-3.0-or-later
// The read-only timeline projection the renderer consumes (arch §16: `docStore` is a normalized
// read-only projection of the engine doc). No engine dependency: fixtures build these directly.
// All times are flicks (`number`, integer). Entities are addressed by id, never by index.
import type { SnapKindName } from "@cutweave/protocol/wasm";
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
  /**
   * Clip height in CSS px (the engine's `track.height`), overriding `size`. Never below the
   * `track-sm` token: at that height or less the track is collapsed (labels only).
   */
  readonly height?: number | null;
}

/** What a sequence marker means (the engine's `MarkerKind`). */
export type MarkerKind = "user" | "beat" | "chapter";

/** A sequence marker, in sequence time (item markers are not drawn yet). */
export interface TimelineMarker {
  readonly id: string;
  /** Where it is, flicks (`>= 0`). */
  readonly t: number;
  /** Length of a range marker, flicks; absent or `null` for a point marker. */
  readonly duration?: number | null;
  readonly label: string;
  /** The marker's own color from the document (`#rrggbb[aa]`); default the marigold token. */
  readonly color?: string;
  readonly kind: MarkerKind;
}

/** A slice of a media asset an item shows (thumbnails, waveform), addressed by media id. */
export interface MediaSlice {
  readonly mediaId: string;
  /** Start of the media range the item plays, flicks (its left edge, unless `reverse`). */
  readonly sourceIn: number;
  /**
   * Media flicks per timeline flick (WP-2D.01; default 1, for display only: the engine's exact
   * fraction decides). `0` shows the frame at `sourceIn` all along (a freeze frame).
   */
  readonly speed?: number;
  /** The item plays its range backwards: its left edge shows the end of the range. */
  readonly reverse?: boolean;
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
  /** Media flicks per timeline flick (WP-2D.01; default 1). */
  readonly speed?: number;
  /** Played backwards: the start edge shows the end of the range (WP-2D.01). */
  readonly reverse?: boolean;
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
  /** Sequence markers (any order; drawn in the ruler). */
  readonly markers?: readonly TimelineMarker[];
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

/**
 * A snap guide: a time (flicks), or the target of a shared snap (`snap(...).target`, WP-2A.07),
 * whose kind picks the guide's color. A bare time draws like an `edge` guide.
 */
export type SnapGuide = number | { readonly kind: SnapKindName; readonly t: number };

/** How the ruler and the playhead flag show time (the Timeline preference "Show time as"). */
export type TimeFormat = "timecode" | "frames" | "seconds";

/** What a video clip's body shows: tiles all along, one tile at its start, or nothing. */
export type ThumbnailMode = "filmstrip" | "first" | "off";

/** What the plain mouse wheel does; Ctrl/Cmd+wheel does the other. */
export type WheelMode = "scroll" | "zoom";

/** How the view keeps the playhead in sight while playing (`followScrollX`). */
export type FollowMode = "page" | "smooth" | "off";

/** Display options the host's preferences set (Settings > Timeline, WP-2A.13). */
export interface TimelineDisplay {
  readonly timeFormat: TimeFormat;
  /** Drop-frame timecode at 29.97 and 59.94 fps; absent: the model's `dropFrame`. */
  readonly dropFrame?: boolean | undefined;
  readonly thumbnails: ThumbnailMode;
  /** Audio clips draw their waveform. */
  readonly waveforms: boolean;
  readonly wheel: WheelMode;
}

export const DEFAULT_DISPLAY: TimelineDisplay = {
  timeFormat: "timecode",
  thumbnails: "filmstrip",
  waveforms: true,
  wheel: "scroll",
};

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
  /** Snap guides, drawn as vertical lines colored by their kind. */
  readonly snapGuides: readonly SnapGuide[];
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
