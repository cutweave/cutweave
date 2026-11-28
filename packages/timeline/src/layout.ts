// SPDX-License-Identifier: GPL-3.0-or-later
// Immutable spatial layout built from a `TimelineModel`: structure-of-arrays item data, per-track
// interval indexes and track geometry. The renderer and hit-testing both read this one structure,
// so what is drawn is what is hit.
import { type IndexList, IntervalIndex } from "./interval-index.ts";
import { type ItemLanes, type KeyframeLane, NO_LANES } from "./keyframes/lanes.ts";
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
  /** Height of one keyframe lane row (the `control-sm` token); absent: lanes take no room. */
  readonly laneRow?: number;
}

const NO_INDICES: readonly number[] = [];

/** The part of a layout that depends on the items only (a lane change reuses it). */
interface ItemIndex {
  markers: readonly TimelineMarker[];
  markerMaxDuration: number;
  start: Float64Array;
  end: Float64Array;
  trackOf: Int32Array;
  flags: Uint8Array;
  contentEnd: number;
  itemIndexById: ReadonlyMap<string, number>;
  indexes: readonly IntervalIndex[];
}

function buildItems(model: TimelineModel, trackIndexById: ReadonlyMap<string, number>): ItemIndex {
  const nTracks = model.tracks.length;
  const n = model.items.length;
  const markers = (model.markers ?? []).slice().sort((a, b) => a.t - b.t);
  const start = new Float64Array(n);
  const end = new Float64Array(n);
  const trackOf = new Int32Array(n);
  const flags = new Uint8Array(n);
  const itemIndexById = new Map<string, number>();
  const perTrack: number[][] = Array.from({ length: nTracks }, () => []);
  let contentEnd = 0;
  for (let i = 0; i < n; i++) {
    const it = model.items[i] as TimelineItem;
    start[i] = it.start;
    end[i] = it.start + it.duration;
    contentEnd = Math.max(contentEnd, it.start + it.duration);
    const ti = trackIndexById.get(it.trackId) ?? -1;
    trackOf[i] = ti;
    let f = 0;
    if (it.state?.disabled) f |= FLAG_DISABLED;
    if (it.state?.proposed) f |= FLAG_PROPOSED;
    if (it.state?.selected) f |= FLAG_SELECTED;
    flags[i] = f;
    itemIndexById.set(it.id, i);
    perTrack[ti]?.push(i);
  }
  return {
    markers,
    markerMaxDuration: markers.reduce((m, k) => Math.max(m, k.duration ?? 0), 0),
    start,
    end,
    trackOf,
    flags,
    contentEnd,
    itemIndexById,
    indexes: perTrack.map((ids) => new IntervalIndex(ids, start, end)),
  };
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

  /** Height of one keyframe lane row, CSS px. */
  readonly laneRow: number;
  /** Per track: the room its open keyframe lanes take under the clips (0: none open). */
  readonly laneExtra: Float64Array;
  /** Per track: the items (indices) whose lanes are open, by start. */
  readonly laneItems: readonly (readonly number[])[];
  /** The open lanes by item index. */
  readonly lanesByItem: ReadonlyMap<number, readonly KeyframeLane[]>;

  /**
   * `lanes`: the open keyframe lanes (WP-2C.05). `from`: a layout of the same model whose item
   * index is reused, so opening a lane or selecting a key rebuilds only the track geometry.
   */
  constructor(
    model: TimelineModel,
    metrics: TrackMetrics,
    lanes: readonly ItemLanes[] = NO_LANES,
    from?: TimelineLayout,
  ) {
    this.model = model;
    this.items = model.items;
    this.pad = metrics.pad;
    this.laneRow = metrics.laneRow ?? 0;
    const nTracks = model.tracks.length;

    let trackIndexById: ReadonlyMap<string, number>;
    let built: ItemIndex;
    if (from !== undefined && from.model === model) {
      trackIndexById = from.trackIndexById;
      built = from;
    } else {
      const byId = new Map<string, number>();
      model.tracks.forEach((t, i) => {
        byId.set(t.id, i);
      });
      trackIndexById = byId;
      built = buildItems(model, byId);
    }
    this.trackIndexById = trackIndexById;
    this.markers = built.markers;
    this.markerMaxDuration = built.markerMaxDuration;
    this.start = built.start;
    this.end = built.end;
    this.trackOf = built.trackOf;
    this.flags = built.flags;
    this.contentEnd = built.contentEnd;
    this.itemIndexById = built.itemIndexById;
    this.indexes = built.indexes;

    // Open lanes: the tallest stack among a track's items is the room the track adds.
    this.laneExtra = new Float64Array(nTracks);
    const lanesByItem = new Map<number, readonly KeyframeLane[]>();
    const laneItems: number[][] = [];
    if (this.laneRow > 0) {
      for (const group of lanes) {
        const i = this.itemIndexById.get(group.itemId);
        const ti = i === undefined ? -1 : (this.trackOf[i] ?? -1);
        if (i === undefined || ti < 0 || group.lanes.length === 0) continue;
        lanesByItem.set(i, group.lanes);
        let list = laneItems[ti];
        if (list === undefined) {
          list = [];
          laneItems[ti] = list;
        }
        list.push(i);
        this.laneExtra[ti] = Math.max(this.laneExtra[ti] ?? 0, group.lanes.length * this.laneRow);
      }
      for (const list of laneItems) {
        list?.sort((p, q) => (this.start[p] ?? 0) - (this.start[q] ?? 0));
      }
    }
    this.lanesByItem = lanesByItem;
    this.laneItems = Array.from({ length: nTracks }, (_, ti) => laneItems[ti] ?? NO_INDICES);

    this.trackTop = new Float64Array(nTracks);
    this.trackHeight = new Float64Array(nTracks);
    this.trackFlags = new Uint8Array(nTracks);
    let y = 0;
    model.tracks.forEach((t, i) => {
      const clip =
        t.height != null && Number.isFinite(t.height)
          ? Math.max(metrics.sizes.sm, Math.round(t.height))
          : (metrics.sizes[t.size ?? "md"] ?? 0);
      const h = clip + 2 * metrics.pad + (this.laneExtra[i] ?? 0);
      this.trackTop[i] = y;
      this.trackHeight[i] = h;
      this.trackFlags[i] = (t.locked ? TRACK_LOCKED : 0) | (t.muted || t.hidden ? TRACK_DIMMED : 0);
      y += h;
    });
    this.totalHeight = y;
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

  /** {@link query} into a reused {@link IndexList} (the draw loop's form). */
  queryInto(track: number, t0: number, t1: number, out: IndexList): IndexList {
    const idx = this.indexes[track];
    if (!idx) {
      out.clear();
      return out;
    }
    return idx.queryInto(t0, t1, out);
  }

  /** Clip height of a track (lane height minus padding and the open keyframe lanes). */
  clipHeight(track: number): number {
    return (this.trackHeight[track] ?? 0) - 2 * this.pad - (this.laneExtra[track] ?? 0);
  }

  /** Whether any keyframe lane is open. */
  get hasLanes(): boolean {
    return this.lanesByItem.size > 0;
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
