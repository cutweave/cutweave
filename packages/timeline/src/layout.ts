// SPDX-License-Identifier: GPL-3.0-or-later
// Immutable spatial layout built from a `TimelineModel`: structure-of-arrays item data, per-track
// interval indexes and track geometry. The renderer and hit-testing both read this one structure,
// so what is drawn is what is hit.
import { IntervalIndex } from "./interval-index.ts";
import type { Rect, TimelineItem, TimelineMarker, TimelineModel, TrackSize } from "./model.ts";
import { FLICKS_PER_SECOND } from "./time.ts";
import type { Viewport } from "./viewport.ts";

export const FLAG_DISABLED = 1;
export const FLAG_PROPOSED = 2;
export const FLAG_SELECTED = 4;

/** Per-track style bits (`TimelineLayout.trackFlags`). */
export const TRACK_LOCKED = 1;
/** Muted (audio) or hidden (video): its clips dim. */
export const TRACK_DIMMED = 2;

/** Clip heights per track size (the `track-*` tokens) and the lane padding above/below a clip. */
export interface TrackMetrics {
  readonly sizes: Readonly<Record<TrackSize, number>>;
  readonly pad: number;
}

export class TimelineLayout {
  readonly model: TimelineModel;
  readonly items: readonly TimelineItem[];
  /** Per item (model order): start, end (flicks) and owning track index (-1 if unknown track). */
  readonly start: Float64Array;
  readonly end: Float64Array;
  readonly trackOf: Int32Array;
  /** `FLAG_*` bits per item from `item.state` (selection from ViewState is applied at draw time). */
  readonly flags: Uint8Array;
  /** Lane top and lane height per track, content space (before scrollY). */
  readonly trackTop: Float64Array;
  readonly trackHeight: Float64Array;
  /** `TRACK_*` bits per track. */
  readonly trackFlags: Uint8Array;
  /** Sequence markers sorted by time, and the longest range marker (flicks) for culling. */
  readonly markers: readonly TimelineMarker[];
  readonly markerMaxDuration: number;
  readonly totalHeight: number;
  /** Largest item end, flicks. */
  readonly contentEnd: number;
  readonly pad: number;
  /** One interval index per track holding item indices (model order). */
  readonly indexes: readonly IntervalIndex[];
  readonly itemIndexById: ReadonlyMap<string, number>;
  readonly trackIndexById: ReadonlyMap<string, number>;

  constructor(model: TimelineModel, metrics: TrackMetrics) {
    this.model = model;
    this.items = model.items;
    this.pad = metrics.pad;
    const nTracks = model.tracks.length;
    const n = model.items.length;

    const trackIndexById = new Map<string, number>();
    model.tracks.forEach((t, i) => {
      trackIndexById.set(t.id, i);
    });
    this.trackIndexById = trackIndexById;

    this.trackTop = new Float64Array(nTracks);
    this.trackHeight = new Float64Array(nTracks);
    this.trackFlags = new Uint8Array(nTracks);
    let y = 0;
    model.tracks.forEach((t, i) => {
      const clip =
        t.height != null && Number.isFinite(t.height)
          ? Math.max(metrics.sizes.sm, Math.round(t.height))
          : (metrics.sizes[t.size ?? "md"] ?? 0);
      const h = clip + 2 * metrics.pad;
      this.trackTop[i] = y;
      this.trackHeight[i] = h;
      this.trackFlags[i] = (t.locked ? TRACK_LOCKED : 0) | (t.muted || t.hidden ? TRACK_DIMMED : 0);
      y += h;
    });
    this.totalHeight = y;

    const markers = (model.markers ?? []).slice().sort((a, b) => a.t - b.t);
    this.markers = markers;
    this.markerMaxDuration = markers.reduce((m, k) => Math.max(m, k.duration ?? 0), 0);

    this.start = new Float64Array(n);
    this.end = new Float64Array(n);
    this.trackOf = new Int32Array(n);
    this.flags = new Uint8Array(n);
    const itemIndexById = new Map<string, number>();
    const perTrack: number[][] = Array.from({ length: nTracks }, () => []);
    let contentEnd = 0;
    for (let i = 0; i < n; i++) {
      const it = model.items[i] as TimelineItem;
      this.start[i] = it.start;
      this.end[i] = it.start + it.duration;
      contentEnd = Math.max(contentEnd, it.start + it.duration);
      const ti = trackIndexById.get(it.trackId) ?? -1;
      this.trackOf[i] = ti;
      let f = 0;
      if (it.state?.disabled) f |= FLAG_DISABLED;
      if (it.state?.proposed) f |= FLAG_PROPOSED;
      if (it.state?.selected) f |= FLAG_SELECTED;
      this.flags[i] = f;
      itemIndexById.set(it.id, i);
      perTrack[ti]?.push(i);
    }
    this.contentEnd = contentEnd;
    this.itemIndexById = itemIndexById;
    this.indexes = perTrack.map((ids) => new IntervalIndex(ids, this.start, this.end));
  }

  get trackCount(): number {
    return this.model.tracks.length;
  }

  /** Index of the track whose lane contains content-space `y`, or -1. */
  trackAtContentY(y: number): number {
    if (y < 0 || y >= this.totalHeight) return -1;
    let lo = 0;
    let hi = this.trackTop.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >>> 1;
      if ((this.trackTop[mid] ?? 0) <= y) lo = mid;
      else hi = mid - 1;
    }
    return lo;
  }

  /** Item indices of `track` overlapping `[t0, t1]` flicks, ascending start, into `out`. */
  query(track: number, t0: number, t1: number, out: number[]): number[] {
    const idx = this.indexes[track];
    if (!idx) {
      out.length = 0;
      return out;
    }
    return idx.query(t0, t1, out);
  }

  /** Clip height of a track (lane height minus padding). */
  clipHeight(track: number): number {
    return (this.trackHeight[track] ?? 0) - 2 * this.pad;
  }

  /** Index of the first marker whose range can reach time `t` (markers sorted by time). */
  firstMarkerFrom(t: number): number {
    const from = t - this.markerMaxDuration;
    const m = this.markers;
    let lo = 0;
    let hi = m.length;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if ((m[mid] as TimelineMarker).t < from) lo = mid + 1;
      else hi = mid;
    }
    return lo;
  }

  /** Bounding box of an item in canvas coordinates, or null for an unknown item index. */
  itemRect(index: number, vp: Viewport): Rect | null {
    const ti = this.trackOf[index];
    if (ti === undefined || ti < 0) return null;
    const k = vp.pxPerSec / FLICKS_PER_SECOND;
    const x = vp.headerWidth + ((this.start[index] ?? 0) - vp.scrollX) * k;
    const w = ((this.end[index] ?? 0) - (this.start[index] ?? 0)) * k;
    const y = vp.rulerHeight + (this.trackTop[ti] ?? 0) - vp.scrollY + this.pad;
    return { x, y, w, h: this.clipHeight(ti) };
  }
}
