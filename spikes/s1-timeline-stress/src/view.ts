// SPDX-License-Identifier: GPL-3.0-or-later
// Viewport state (pan/zoom), track layout, hit-testing and selection.
import { type Kind, type Timeline, TRACK_KINDS } from "./model.ts";
import type { Theme } from "./theme.ts";

export const MIN_PPS = 0.4;
export const MAX_PPS = 2000;

export type Hit =
  | { type: "clip"; clip: number }
  | { type: "keyframe"; clip: number; kf: number }
  | null;

export class View {
  /** Seconds at the left edge of the canvas. */
  t0 = 0;
  /** Pixels per second. */
  pps = 20;
  /** Vertical scroll of the track area in CSS px. */
  y0 = 0;
  width = 1;
  height = 1;
  playhead = 30;
  playing = false;
  readonly selectedClips = new Set<number>();
  selectedKeyframe = -1;
  marquee: { x0: number; y0: number; x1: number; y1: number } | null = null;

  readonly rulerH: number;
  readonly trackTop: number[] = [];
  readonly trackH: number[] = [];
  readonly contentH: number;
  readonly keyframeRowFromBottom: number;
  readonly keyframeHitPx: number;
  private readonly scratch: number[] = [];

  constructor(
    readonly tl: Timeline,
    readonly theme: Theme,
  ) {
    this.rulerH = theme.trackSm;
    let y = 0;
    for (const k of TRACK_KINDS) {
      const h = k === "media" ? theme.trackLg : theme.trackMd;
      this.trackTop.push(y);
      this.trackH.push(h);
      y += h;
    }
    this.contentH = y;
    this.keyframeRowFromBottom = theme.space2 + theme.space1;
    this.keyframeHitPx = theme.space2;
  }

  get t1(): number {
    return this.t0 + this.width / this.pps;
  }
  get maxY0(): number {
    return Math.max(0, this.contentH - (this.height - this.rulerH));
  }
  kindOf(track: number): Kind {
    return TRACK_KINDS[track] ?? "media";
  }
  timeToX(t: number): number {
    return (t - this.t0) * this.pps;
  }
  xToTime(x: number): number {
    return this.t0 + x / this.pps;
  }

  clamp(): void {
    this.pps = Math.min(MAX_PPS, Math.max(MIN_PPS, this.pps));
    const span = this.width / this.pps;
    this.t0 = Math.min(this.tl.duration - span * 0.1, Math.max(-span * 0.9, this.t0));
    this.y0 = Math.min(this.maxY0, Math.max(0, this.y0));
  }

  panBy(dxPx: number, dyPx: number): void {
    this.t0 += dxPx / this.pps;
    this.y0 += dyPx;
    this.clamp();
  }

  /** Zoom keeping the time under `anchorX` fixed. */
  zoomAt(anchorX: number, factor: number): void {
    const t = this.xToTime(anchorX);
    this.pps *= factor;
    this.pps = Math.min(MAX_PPS, Math.max(MIN_PPS, this.pps));
    this.t0 = t - anchorX / this.pps;
    this.clamp();
  }

  /** Track row under a canvas-space y (below the ruler), or -1. */
  trackAtY(y: number): number {
    const c = y - this.rulerH + this.y0;
    if (c < 0 || c >= this.contentH) return -1;
    let lo = 0;
    let hi = this.trackTop.length - 1;
    while (lo < hi) {
      const mid = (lo + hi + 1) >>> 1;
      if ((this.trackTop[mid] ?? 0) <= c) lo = mid;
      else hi = mid - 1;
    }
    return lo;
  }

  trackScreenY(track: number): number {
    return this.rulerH + (this.trackTop[track] ?? 0) - this.y0;
  }

  /** Keyframe hit first (it sits on top of the clip), then clip body. */
  hitTest(x: number, y: number): Hit {
    const tr = this.trackAtY(y);
    if (tr < 0) return null;
    const t = this.xToTime(x);
    const pad = 0.5 / this.pps;
    const clips = this.tl.index[tr]?.query(t - pad, t + pad, this.scratch);
    if (!clips || clips.length === 0) return null;
    let found = -1;
    for (const c of clips) {
      if ((this.tl.start[c] ?? 0) <= t + pad && (this.tl.end[c] ?? 0) >= t - pad) found = c;
    }
    if (found < 0) return null;
    const bottom = this.trackScreenY(tr) + (this.trackH[tr] ?? 0);
    const rowY = bottom - this.keyframeRowFromBottom;
    if (Math.abs(y - rowY) <= this.keyframeHitPx && this.keyframesVisible(found)) {
      const kf = this.nearestKeyframe(found, t, this.keyframeHitPx / this.pps);
      if (kf >= 0) return { type: "keyframe", clip: found, kf };
    }
    return { type: "clip", clip: found };
  }

  /** Same LOD rule the renderer uses so what is hittable is what is drawn. */
  keyframesVisible(clip: number): boolean {
    return ((this.tl.end[clip] ?? 0) - (this.tl.start[clip] ?? 0)) * this.pps >= KEYFRAME_MIN_PX;
  }

  /** Global keyframe index nearest to t within tolerance, or -1 (binary search). */
  nearestKeyframe(clip: number, t: number, tol: number): number {
    const off = this.tl.kfOff[clip] ?? 0;
    let lo = off;
    let hi = off + (this.tl.kfCount[clip] ?? 0);
    const end = hi;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if ((this.tl.kfTime[mid] ?? 0) < t) lo = mid + 1;
      else hi = mid;
    }
    let best = -1;
    let bestD = tol;
    for (const i of [lo - 1, lo]) {
      if (i < off || i >= end) continue;
      const d = Math.abs((this.tl.kfTime[i] ?? 0) - t);
      if (d <= bestD) {
        bestD = d;
        best = i;
      }
    }
    return best;
  }

  /** Marquee selection: clips in tracks/time range via the per-track index. */
  marqueeSelect(x0: number, y0: number, x1: number, y1: number, additive: boolean): void {
    if (!additive) this.selectedClips.clear();
    const ta = this.xToTime(Math.min(x0, x1));
    const tb = this.xToTime(Math.max(x0, x1));
    const ya = Math.min(y0, y1);
    const yb = Math.max(y0, y1);
    for (let tr = 0; tr < this.trackTop.length; tr++) {
      const top = this.trackScreenY(tr);
      if (top + (this.trackH[tr] ?? 0) < ya || top > yb) continue;
      for (const c of this.tl.index[tr]?.query(ta, tb, this.scratch) ?? []) {
        this.selectedClips.add(c);
      }
    }
  }

  select(hit: Hit, additive: boolean): void {
    if (!additive) {
      this.selectedClips.clear();
      this.selectedKeyframe = -1;
    }
    if (!hit) return;
    if (hit.type === "keyframe") this.selectedKeyframe = hit.kf;
    else if (additive && this.selectedClips.has(hit.clip)) this.selectedClips.delete(hit.clip);
    else this.selectedClips.add(hit.clip);
  }
}

/** LOD thresholds (CSS px of clip width). */
export const KEYFRAME_MIN_PX = 100;
export const THUMB_MIN_PX = 48;
export const LABEL_MIN_PX = 70;
