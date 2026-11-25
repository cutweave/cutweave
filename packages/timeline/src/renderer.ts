// SPDX-License-Identifier: GPL-3.0-or-later
// Virtualized Canvas2D renderer (arch §16). Draws only what intersects the (dirty) region:
//  - per track, the interval index yields just the visible clips; tracks outside the viewport are
//    skipped; nothing is ever iterated globally;
//  - draw calls are batched per track and clip kind: one path for all bodies, one for the header
//    strips, one for keyframes, one for waveform bars, one for selection outlines;
//  - level of detail: labels, tiles, waveforms and keyframes appear only when the clip is wide
//    enough (geometry.ts, shared with hit-testing); label count is capped per frame;
//  - no per-clip allocation in the hot loop (reused id buffers and path batches).
// No literal colors: everything comes from the `Theme` resolved from the design tokens.
import { bucketPxPerSec, type MediaCaches, zoomBucket } from "./cache.ts";
import {
  BODY_MEDIA_MIN_PX,
  HEADER_TOGGLES,
  type HeaderToggle,
  KEYFRAME_MIN_PX,
  keyframeRowY,
  LABEL_MIN_PX,
  labelBand,
  MAX_LABELS,
  nameRect,
  showsKeyframes,
  TILE_ASPECT,
  toggleRect,
} from "./geometry.ts";
import { FLAG_DISABLED, FLAG_PROPOSED, FLAG_SELECTED, type TimelineLayout } from "./layout.ts";
import type { ItemKind, Rect, TimelineTrack, ViewState } from "./model.ts";
import { chooseTickPlan } from "./ruler.ts";
import type { Theme } from "./theme.ts";
import { FLICKS_PER_SECOND, type FrameRate, formatTimecode } from "./time.ts";
import { type Viewport, xToTime } from "./viewport.ts";

const KIND_INDEX: Readonly<Record<ItemKind, number>> = {
  media: 0,
  audio: 1,
  graphics: 2,
  captions: 3,
  effects: 4,
};
const KINDS: readonly ItemKind[] = ["media", "audio", "graphics", "captions", "effects"];
/** Waveform bar spacing, CSS px. */
const WAVE_STEP = 2;

// 24x24 icon geometry (the same glyphs as the design-system components). Drawn as stroked Path2D.
const ICON_RECT = (x: number, y: number, w: number, h: number, r: number): string =>
  `M${x + r} ${y}h${w - 2 * r}a${r} ${r} 0 0 1 ${r} ${r}v${h - 2 * r}a${r} ${r} 0 0 1-${r} ${r}h-${w - 2 * r}a${r} ${r} 0 0 1-${r}-${r}v-${h - 2 * r}a${r} ${r} 0 0 1 ${r}-${r}z`;
const KIND_ICON: Readonly<Record<ItemKind, string>> = {
  media: `${ICON_RECT(3, 3, 18, 18, 2)}M7 3v18M17 3v18`,
  audio: "M11 5 6 9H2v6h4l5 4zM15.5 8.5a5 5 0 0 1 0 7",
  graphics: "M4 7V5h16v2M9 19h6M12 5v14",
  captions: `${ICON_RECT(3, 5, 18, 14, 2)}M7 15h4M15 15h2`,
  effects: "M15 4V2M15 16v-2M8 9h2M20 9h2M3 21l9-9",
};
const TOGGLE_ICON: Readonly<Record<HeaderToggle, string>> = {
  lock: `${ICON_RECT(3, 11, 18, 11, 2)}M7 11V7a5 5 0 0 1 10 0v4`,
  mute: "M11 5 6 9H2v6h4l5 4zM22 9l-6 6M16 9l6 6",
  hide: "M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7-10-7-10-7zM15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0",
};

export interface FrameStats {
  drawCalls: number;
  /** Clips that passed the per-track visibility query (what the index returned). */
  visibleItems: number;
  visibleKeyframes: number;
  tiles: number;
  labels: number;
  tracksDrawn: number;
}

export interface FrameInput {
  readonly layout: TimelineLayout;
  readonly vp: Viewport;
  readonly theme: Theme;
  readonly fps: FrameRate;
  readonly dropFrame: boolean;
  readonly view: Pick<ViewState, "playhead" | "selection" | "snapGuides" | "range" | "marquee">;
  readonly caches: MediaCaches;
  readonly dpr: number;
  /** Item index under the pointer (trim handles show on hover), or -1. */
  readonly hoverItem: number;
  /** Redraw only this canvas-space rectangle (CSS px); null redraws everything. */
  readonly region: Rect | null;
}

export class TimelineRenderer {
  readonly stats: FrameStats = {
    drawCalls: 0,
    visibleItems: 0,
    visibleKeyframes: 0,
    tiles: 0,
    labels: 0,
    tracksDrawn: 0,
  };
  /** Half width of the playhead flag as last drawn; the hit-test uses it. */
  flagHalfWidth = 0;

  /**
   * Horizontal span (canvas CSS px) to repaint for a playhead at canvas x `phx`: the line, the
   * flag where it is drawn (kept inside the lane area) and every ruler label the flag hides, so
   * moving the playhead redraws those labels whole.
   */
  playheadSpan(phx: number, vp: Viewport, theme: Theme): { x0: number; x1: number } {
    const fw = 2 * (this.flagHalfWidth || 4 * theme.space4);
    const fx = flagX(phx, fw, vp);
    const reach = this.timecodeWidth + theme.space1 + 2;
    return { x0: Math.min(phx, fx) - reach, x1: Math.max(phx, fx + fw) + reach };
  }

  private readonly ids: number[] = [];
  private readonly buckets: number[][] = Array.from({ length: 15 }, () => []);
  private readonly selected: number[] = [];
  private readonly paths = new Map<string, Path2D>();
  private metricsFor: Theme | null = null;
  private charCaption = 6;
  private charLabel = 7;
  private timecodeWidth = 80;
  private labelsLeft = 0;

  /** Forget cached text metrics (call after fonts load or the theme changes). */
  resetMetrics(): void {
    this.metricsFor = null;
  }

  private measure(ctx: CanvasRenderingContext2D, th: Theme): void {
    if (this.metricsFor === th) return;
    this.metricsFor = th;
    const sample = "abcdefghijklmnopqrstuvwxyz0123456789";
    ctx.font = th.fontCaption;
    this.charCaption = ctx.measureText(sample).width / sample.length || 6;
    ctx.font = th.fontLabel;
    this.charLabel = ctx.measureText(sample).width / sample.length || 7;
    ctx.font = th.fontNumeric;
    this.timecodeWidth = ctx.measureText("00:00:00:00").width || 80;
    this.paths.clear();
  }

  private path(svg: string): Path2D | null {
    if (typeof Path2D === "undefined") return null;
    let p = this.paths.get(svg);
    if (!p) {
      p = new Path2D(svg);
      this.paths.set(svg, p);
    }
    return p;
  }

  private glyph(
    ctx: CanvasRenderingContext2D,
    svg: string,
    cx: number,
    cy: number,
    size: number,
    color: string,
  ): void {
    const p = this.path(svg);
    if (!p) return;
    const s = size / 24;
    ctx.save();
    ctx.translate(cx - size / 2, cy - size / 2);
    ctx.scale(s, s);
    ctx.strokeStyle = color;
    ctx.lineWidth = 2;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    ctx.stroke(p);
    ctx.restore();
    this.stats.drawCalls++;
  }

  private fit(text: string, maxPx: number, charPx: number): string {
    const maxChars = Math.floor(maxPx / charPx);
    if (text.length <= maxChars) return text;
    return maxChars <= 1 ? "" : `${text.slice(0, maxChars - 1)}…`;
  }

  draw(ctx: CanvasRenderingContext2D, f: FrameInput): void {
    const { vp, theme: th, layout } = f;
    const s = this.stats;
    s.drawCalls = s.visibleItems = s.visibleKeyframes = s.tiles = s.labels = s.tracksDrawn = 0;
    this.labelsLeft = MAX_LABELS;
    this.measure(ctx, th);

    ctx.setTransform(f.dpr, 0, 0, f.dpr, 0, 0);
    ctx.save();
    const full: Rect = { x: 0, y: 0, w: vp.width, h: vp.height };
    const reg = f.region ? intersect(f.region, full) : full;
    // Only a full frame asks for every visible tile, so only it may cancel loads that scrolled away.
    f.caches.beginFrame(reg.x <= 0 && reg.y <= 0 && reg.w >= full.w && reg.h >= full.h);
    if (reg.w <= 0 || reg.h <= 0) {
      ctx.restore();
      return;
    }
    ctx.beginPath();
    ctx.rect(reg.x, reg.y, reg.w, reg.h);
    ctx.clip();
    ctx.fillStyle = th.surface1;
    ctx.fillRect(reg.x, reg.y, reg.w, reg.h);
    s.drawCalls += 2;

    const laneX0 = Math.max(reg.x, vp.headerWidth);
    const laneX1 = reg.x + reg.w;
    const laneY0 = Math.max(reg.y, vp.rulerHeight);
    const laneY1 = reg.y + reg.h;
    const k = vp.pxPerSec / FLICKS_PER_SECOND;

    if (laneX1 > laneX0 && laneY1 > laneY0) {
      // One flick of margin so clips touching the edge are not culled by rounding.
      const t0 = xToTime(vp, laneX0) - 1;
      const t1 = xToTime(vp, laneX1) + 1;
      const tracks = layout.model.tracks;
      for (let ti = 0; ti < tracks.length; ti++) {
        const laneTop = vp.rulerHeight + (layout.trackTop[ti] ?? 0) - vp.scrollY;
        const laneH = layout.trackHeight[ti] ?? 0;
        if (laneTop + laneH <= laneY0 || laneTop >= laneY1) continue; // vertical virtualization
        s.tracksDrawn++;
        ctx.fillStyle = ti % 2 === 0 ? th.lane : th.laneAlt;
        ctx.fillRect(vp.headerWidth, laneTop, vp.width - vp.headerWidth, laneH);
        s.drawCalls++;
        this.drawTrack(ctx, f, ti, tracks[ti] as TimelineTrack, laneTop, t0, t1, laneX0, laneX1, k);
      }
      this.drawOverlays(ctx, f, laneX0, laneX1, k);
    }

    if (reg.x < vp.headerWidth && laneY1 > laneY0) this.drawHeaders(ctx, f, laneY0, laneY1);
    if (reg.y < vp.rulerHeight) this.drawRuler(ctx, f, laneX0, laneX1, k);
    // Header column rule.
    ctx.fillStyle = th.borderSubtle;
    ctx.fillRect(vp.headerWidth - th.borderThin, 0, th.borderThin, vp.height);
    s.drawCalls++;
    ctx.restore();
  }

  // ---- tracks and clips -------------------------------------------------------------------

  private drawTrack(
    ctx: CanvasRenderingContext2D,
    f: FrameInput,
    ti: number,
    track: TimelineTrack,
    laneTop: number,
    t0: number,
    t1: number,
    rx0: number,
    rx1: number,
    k: number,
  ): void {
    const { layout, vp, theme: th, view } = f;
    const s = this.stats;
    const ids = layout.query(ti, t0, t1, this.ids);
    if (ids.length === 0) return;
    s.visibleItems += ids.length;
    const cy = laneTop + layout.pad;
    const ch = layout.clipHeight(ti);

    // Partition by (state group, kind) so every bucket is one batch.
    for (const b of this.buckets) b.length = 0;
    this.selected.length = 0;
    const sel = view.selection;
    for (let n = 0; n < ids.length; n++) {
      const i = ids[n] as number;
      const fl = layout.flags[i] ?? 0;
      const item = layout.items[i];
      if (!item) continue;
      const group = fl & FLAG_PROPOSED ? 2 : fl & FLAG_DISABLED ? 1 : 0;
      (this.buckets[group * 5 + KIND_INDEX[item.kind]] as number[]).push(i);
      if (fl & FLAG_SELECTED || (sel.size > 0 && sel.has(item.id))) this.selected.push(i);
    }

    for (let group = 0; group < 3; group++) {
      ctx.globalAlpha = group === 1 ? th.opacityDisabled : 1;
      for (let kind = 0; kind < 5; kind++) {
        const bucket = this.buckets[group * 5 + kind] as number[];
        if (bucket.length > 0) {
          this.drawBucket(ctx, f, bucket, group, KINDS[kind] as ItemKind, cy, ch, rx0, rx1, k);
        }
      }
    }
    ctx.globalAlpha = 1;

    if (showsKeyframes(ch, th)) this.drawKeyframes(ctx, f, ids, cy, ch, rx0, rx1, k);

    if (track.locked) {
      // Locked lane dims: the lane color at half strength over the clips.
      ctx.globalAlpha = th.opacityDisabled;
      ctx.fillStyle = ti % 2 === 0 ? th.lane : th.laneAlt;
      ctx.fillRect(vp.headerWidth, laneTop, vp.width - vp.headerWidth, layout.trackHeight[ti] ?? 0);
      ctx.globalAlpha = 1;
      s.drawCalls++;
    }

    this.drawSelection(ctx, f, ti, cy, ch, k);
  }

  private drawBucket(
    ctx: CanvasRenderingContext2D,
    f: FrameInput,
    bucket: readonly number[],
    group: number,
    kind: ItemKind,
    cy: number,
    ch: number,
    rx0: number,
    rx1: number,
    k: number,
  ): void {
    const { layout, vp, theme: th } = f;
    const s = this.stats;
    const col = th.clip[kind];
    const proposed = group === 2;
    const r = th.radiusSm;
    const bw = th.borderThin;

    // Bodies: one rounded rect per clip in one path (fill, then 1px border stroke).
    ctx.beginPath();
    for (let n = 0; n < bucket.length; n++) {
      const i = bucket[n] as number;
      const x = this.edgeX(layout, vp, i, k, 0);
      const w = this.edgeX(layout, vp, i, k, 1) - x;
      ctx.roundRect(x + bw / 2, cy + bw / 2, Math.max(1, w - bw), ch - bw, r);
    }
    ctx.fillStyle = proposed ? th.agentSubtle : col.body;
    ctx.fill();
    ctx.strokeStyle = proposed ? th.agent : col.strong;
    ctx.lineWidth = bw;
    if (proposed) ctx.setLineDash([th.space1, th.space1]);
    ctx.stroke();
    if (proposed) {
      ctx.setLineDash([]);
      // Diagonal hatch inside the proposed clips (clip to the same batched path).
      ctx.save();
      ctx.clip();
      ctx.beginPath();
      const span = ch + th.space2;
      for (let hx = Math.floor(rx0 / th.space2) * th.space2 - span; hx < rx1; hx += th.space2) {
        ctx.moveTo(hx, cy + ch);
        ctx.lineTo(hx + ch, cy);
      }
      ctx.strokeStyle = th.agentSubtle;
      ctx.lineWidth = th.space1;
      ctx.stroke();
      ctx.restore();
      s.drawCalls += 2;
    }

    // Header strips: 3px lines in the strong color, inset from the rounded corners.
    ctx.beginPath();
    for (let n = 0; n < bucket.length; n++) {
      const i = bucket[n] as number;
      const x = this.edgeX(layout, vp, i, k, 0);
      const w = this.edgeX(layout, vp, i, k, 1) - x;
      if (w < 2 * r + 1) continue;
      ctx.moveTo(x + r / 2, cy + th.clipStrip / 2);
      ctx.lineTo(x + w - r / 2, cy + th.clipStrip / 2);
    }
    ctx.strokeStyle = proposed ? th.agent : col.strong;
    ctx.lineWidth = th.clipStrip;
    ctx.lineCap = "butt";
    ctx.stroke();
    s.drawCalls += 4;

    // Body area under the label band: tiles and waveform.
    const band = labelBand(th);
    const areaY = cy + th.clipStrip + band;
    const areaH = cy + ch - bw - areaY;
    const bodyOk = areaH >= th.space4;
    if (bodyOk) {
      if (f.caches.hasFilmstripLoader) this.drawTiles(ctx, f, bucket, areaY, areaH, rx0, rx1, k);
      if (kind === "audio" && f.caches.hasWaveformLoader) {
        this.drawWaveforms(ctx, f, bucket, col.strong, areaY, areaH, rx0, rx1, k);
      }
    }

    // Labels.
    ctx.fillStyle = th.textPrimary;
    ctx.font = th.fontCaption;
    ctx.textBaseline = "middle";
    ctx.textAlign = "left";
    for (let n = 0; n < bucket.length && this.labelsLeft > 0; n++) {
      const i = bucket[n] as number;
      const x = this.edgeX(layout, vp, i, k, 0);
      const w = this.edgeX(layout, vp, i, k, 1) - x;
      if (w < LABEL_MIN_PX) continue;
      const lx = Math.max(x, vp.headerWidth) + th.space2;
      const avail = x + w - th.space2 - lx;
      if (avail < th.space4) continue;
      const text = this.fit(layout.items[i]?.label ?? "", avail, this.charCaption);
      if (text === "") continue;
      ctx.fillText(text, lx, cy + th.clipStrip + band / 2);
      this.labelsLeft--;
      s.labels++;
      s.drawCalls++;
    }
  }

  /** Left (`side` 0) or right (1) edge x of item `i`, rounded to a whole CSS pixel. */
  private edgeX(layout: TimelineLayout, vp: Viewport, i: number, k: number, side: 0 | 1): number {
    const t = side === 0 ? (layout.start[i] ?? 0) : (layout.end[i] ?? 0);
    const x = Math.round(vp.headerWidth + (t - vp.scrollX) * k);
    if (side === 0) return x;
    const left = Math.round(vp.headerWidth + ((layout.start[i] ?? 0) - vp.scrollX) * k);
    return Math.max(x, left + 1); // at least one pixel wide
  }

  private drawTiles(
    ctx: CanvasRenderingContext2D,
    f: FrameInput,
    bucket: readonly number[],
    areaY: number,
    areaH: number,
    rx0: number,
    rx1: number,
    k: number,
  ): void {
    const { layout, vp, theme: th, caches } = f;
    const bk = zoomBucket(vp.pxPerSec);
    const tilePx = Math.round(areaH * TILE_ASPECT);
    const dur = Math.max(1, Math.round((tilePx / bucketPxPerSec(bk)) * FLICKS_PER_SECOND));
    let placeholders = false;
    ctx.beginPath();
    for (let n = 0; n < bucket.length; n++) {
      const i = bucket[n] as number;
      const item = layout.items[i];
      const slice = item?.thumbnails;
      if (!item || !slice) continue;
      const x = vp.headerWidth + ((layout.start[i] ?? 0) - vp.scrollX) * k;
      const w = ((layout.end[i] ?? 0) - (layout.start[i] ?? 0)) * k;
      if (w < BODY_MEDIA_MIN_PX) continue;
      const v0 = Math.max(x, rx0, vp.headerWidth);
      const v1 = Math.min(x + w, rx1);
      if (v1 <= v0) continue;
      const j0 = Math.floor((slice.sourceIn + (v0 - x) / k) / dur);
      const j1 = Math.floor((slice.sourceIn + (v1 - x) / k) / dur);
      for (let j = j0; j <= j1 && j - j0 < 256; j++) {
        const tx = x + (j * dur - slice.sourceIn) * k;
        const tw = dur * k;
        const d0 = Math.max(tx, v0);
        const d1 = Math.min(tx + tw, v1);
        if (d1 <= d0) continue;
        const bmp = caches.tile(`${slice.mediaId}|${bk}|${tilePx}x${areaH}|${j}`, {
          mediaId: slice.mediaId,
          sourceTime: j * dur,
          tileDuration: dur,
          width: tilePx,
          height: areaH,
        });
        if (!bmp) {
          ctx.rect(d0, areaY, d1 - d0, areaH);
          placeholders = true;
          continue;
        }
        const sx = ((d0 - tx) / tw) * bmp.width;
        const sw = ((d1 - d0) / tw) * bmp.width;
        ctx.drawImage(bmp, sx, 0, sw, bmp.height, d0, areaY, d1 - d0, areaH);
        this.stats.tiles++;
        this.stats.drawCalls++;
      }
    }
    if (placeholders) {
      ctx.save();
      ctx.globalAlpha *= 0.5;
      ctx.fillStyle = th.surface3;
      ctx.fill();
      ctx.restore();
      this.stats.drawCalls++;
    }
  }

  private drawWaveforms(
    ctx: CanvasRenderingContext2D,
    f: FrameInput,
    bucket: readonly number[],
    color: string,
    areaY: number,
    areaH: number,
    rx0: number,
    rx1: number,
    k: number,
  ): void {
    const { layout, vp, caches } = f;
    const bk = zoomBucket(vp.pxPerSec);
    const wantPps = Math.ceil(bucketPxPerSec(bk) * 2);
    const mid = areaY + areaH / 2;
    const half = areaH / 2 - 1;
    const pxPerFlick = k;
    let any = false;
    ctx.beginPath();
    for (let n = 0; n < bucket.length; n++) {
      const i = bucket[n] as number;
      const slice = layout.items[i]?.waveform;
      if (!slice) continue;
      const x = vp.headerWidth + ((layout.start[i] ?? 0) - vp.scrollX) * k;
      const w = ((layout.end[i] ?? 0) - (layout.start[i] ?? 0)) * k;
      if (w < BODY_MEDIA_MIN_PX) continue;
      const peaks = caches.peaks(`${slice.mediaId}|${bk}`, {
        mediaId: slice.mediaId,
        peaksPerSecond: wantPps,
      });
      if (!peaks) continue;
      const c0 = Math.max(Math.floor(x), Math.floor(rx0), vp.headerWidth);
      const c1 = Math.min(x + w, rx1);
      const P = peaks.peaksPerSecond;
      const nPeaks = peaks.max.length;
      for (let c = c0; c < c1; c += WAVE_STEP) {
        const srcA = (slice.sourceIn + (c - x) / pxPerFlick) / FLICKS_PER_SECOND;
        const a = Math.floor(srcA * P);
        const b = Math.max(a, Math.floor((srcA + WAVE_STEP / vp.pxPerSec) * P));
        if (b < 0 || a >= nPeaks) continue;
        const stride = Math.max(1, Math.floor((b - a) / 16));
        let lo = 0;
        let hi = 0;
        for (let p = Math.max(0, a); p <= b && p < nPeaks; p += stride) {
          const mn = peaks.min[p] ?? 0;
          const mx = peaks.max[p] ?? 0;
          if (mn < lo) lo = mn;
          if (mx > hi) hi = mx;
        }
        const top = mid - hi * half;
        const bottom = Math.max(mid - lo * half, top + 1);
        ctx.moveTo(c + 0.5, top);
        ctx.lineTo(c + 0.5, bottom);
        any = true;
      }
    }
    if (any) {
      ctx.strokeStyle = color;
      ctx.lineWidth = 1;
      ctx.stroke();
      this.stats.drawCalls++;
    }
  }

  private drawKeyframes(
    ctx: CanvasRenderingContext2D,
    f: FrameInput,
    ids: readonly number[],
    cy: number,
    ch: number,
    rx0: number,
    rx1: number,
    k: number,
  ): void {
    const { layout, vp, theme: th } = f;
    const rowY = keyframeRowY(th, cy, ch);
    const r = th.space1;
    const ta = xToTime(vp, rx0);
    const tb = xToTime(vp, rx1);
    let any = false;
    ctx.beginPath();
    for (let n = 0; n < ids.length; n++) {
      const i = ids[n] as number;
      const kfs = layout.items[i]?.keyframes;
      if (!kfs || kfs.length === 0) continue;
      if (((layout.end[i] ?? 0) - (layout.start[i] ?? 0)) * k < KEYFRAME_MIN_PX) continue;
      let lo = 0;
      let hi = kfs.length;
      while (lo < hi) {
        const mid = (lo + hi) >>> 1;
        if ((kfs[mid] ?? 0) < ta) lo = mid + 1;
        else hi = mid;
      }
      for (let j = lo; j < kfs.length; j++) {
        const kt = kfs[j] as number;
        if (kt > tb) break;
        const x = vp.headerWidth + (kt - vp.scrollX) * k;
        ctx.moveTo(x, rowY - r);
        ctx.lineTo(x + r, rowY);
        ctx.lineTo(x, rowY + r);
        ctx.lineTo(x - r, rowY);
        ctx.closePath();
        this.stats.visibleKeyframes++;
        any = true;
      }
    }
    if (any) {
      ctx.fillStyle = th.textPrimary;
      ctx.fill();
      this.stats.drawCalls++;
    }
  }

  private drawSelection(
    ctx: CanvasRenderingContext2D,
    f: FrameInput,
    ti: number,
    cy: number,
    ch: number,
    k: number,
  ): void {
    const { layout, vp, theme: th } = f;
    const hover = f.hoverItem;
    const hoverHere = hover >= 0 && layout.trackOf[hover] === ti && !this.selected.includes(hover);
    if (this.selected.length === 0 && !hoverHere) return;
    // Outline: 2px marigold, inside the clip box. Selection is the outline only, never a tint.
    ctx.beginPath();
    for (const i of this.selected) {
      const x = this.edgeX(layout, vp, i, k, 0);
      const w = this.edgeX(layout, vp, i, k, 1) - x;
      const o = th.borderThin;
      ctx.roundRect(
        x + o,
        cy + o,
        Math.max(1, w - 2 * o),
        ch - 2 * o,
        Math.max(0, th.radiusSm - o),
      );
    }
    ctx.strokeStyle = th.marigold;
    ctx.lineWidth = 2;
    ctx.stroke();
    this.stats.drawCalls++;
    // Trim handles on selected (and hovered) clips: short marigold bars inside each edge.
    ctx.beginPath();
    const bar = (i: number): void => {
      const x = this.edgeX(layout, vp, i, k, 0);
      const w = this.edgeX(layout, vp, i, k, 1) - x;
      if (w < 6 * th.space1) return;
      const y0 = cy + ch * 0.3;
      const y1 = cy + ch * 0.7;
      const inset = th.space1;
      ctx.moveTo(x + inset, y0);
      ctx.lineTo(x + inset, y1);
      ctx.moveTo(x + w - inset, y0);
      ctx.lineTo(x + w - inset, y1);
    };
    for (const i of this.selected) bar(i);
    if (hoverHere) bar(hover);
    ctx.strokeStyle = th.marigold;
    ctx.lineWidth = th.space0_5 + 1;
    ctx.lineCap = "round";
    ctx.stroke();
    ctx.lineCap = "butt";
    this.stats.drawCalls++;
  }

  // ---- overlays (range shade, snap guides, marquee, playhead line) ------------------------

  private drawOverlays(
    ctx: CanvasRenderingContext2D,
    f: FrameInput,
    rx0: number,
    rx1: number,
    k: number,
  ): void {
    const { vp, theme: th, view } = f;
    const top = vp.rulerHeight;
    const bottom = vp.height;
    const px = (t: number): number => vp.headerWidth + (t - vp.scrollX) * k;

    if (view.range) {
      const a = Math.round(px(view.range.in));
      const b = Math.round(px(view.range.out));
      ctx.fillStyle = th.marigoldSubtle;
      ctx.fillRect(a, top, b - a, bottom - top);
      this.stats.drawCalls++;
    }

    if (view.snapGuides.length > 0) {
      ctx.beginPath();
      for (const t of view.snapGuides) {
        const x = Math.round(px(t)) + 0.5;
        if (x < rx0 - 1 || x > rx1 + 1 || x < vp.headerWidth) continue;
        ctx.moveTo(x, top);
        ctx.lineTo(x, bottom);
      }
      ctx.strokeStyle = th.textSecondary;
      ctx.lineWidth = th.borderThin;
      ctx.setLineDash([th.space1, th.space1]);
      ctx.stroke();
      ctx.setLineDash([]);
      this.stats.drawCalls++;
    }

    const m = view.marquee;
    if (m) {
      ctx.fillStyle = th.marigoldSubtle;
      ctx.fillRect(m.x, m.y, m.w, m.h);
      ctx.strokeStyle = th.marigold;
      ctx.lineWidth = th.borderThin;
      ctx.strokeRect(m.x + 0.5, m.y + 0.5, m.w - 1, m.h - 1);
      this.stats.drawCalls += 2;
    }

    const x = Math.round(px(view.playhead));
    if (x >= vp.headerWidth - 1 && x <= vp.width + 1) {
      ctx.fillStyle = th.playhead;
      ctx.fillRect(x - th.space0_5 / 2, top, th.space0_5, bottom - top);
      this.stats.drawCalls++;
    }
  }

  // ---- headers ----------------------------------------------------------------------------

  private drawHeaders(ctx: CanvasRenderingContext2D, f: FrameInput, y0: number, y1: number): void {
    const { layout, vp, theme: th } = f;
    const tracks = layout.model.tracks;
    ctx.textBaseline = "middle";
    ctx.textAlign = "left";
    for (let ti = 0; ti < tracks.length; ti++) {
      const laneTop = vp.rulerHeight + (layout.trackTop[ti] ?? 0) - vp.scrollY;
      const laneH = layout.trackHeight[ti] ?? 0;
      if (laneTop + laneH <= y0 || laneTop >= y1) continue;
      const track = tracks[ti] as TimelineTrack;
      ctx.fillStyle = th.surface1;
      ctx.fillRect(0, laneTop, vp.headerWidth, laneH);
      ctx.fillStyle = th.borderSubtle;
      ctx.fillRect(0, laneTop + laneH - th.borderThin, vp.headerWidth, th.borderThin);
      if (track.main) {
        ctx.fillStyle = th.marigold;
        ctx.fillRect(0, laneTop, th.space0_5, laneH);
      }
      const cy = laneTop + laneH / 2;
      this.glyph(
        ctx,
        KIND_ICON[track.kind],
        th.space2 + th.iconSm / 2,
        cy,
        th.iconSm,
        th.clip[track.kind].strong,
      );

      const nameBox = nameRect(vp, th, laneTop, laneH);
      const nameX = nameBox.x;
      const text = this.fit(track.name, nameBox.w, this.charLabel);
      ctx.font = th.fontLabel;
      ctx.fillStyle = th.textPrimary;
      if (text !== "") ctx.fillText(text, nameX, cy);
      this.stats.drawCalls += 4;

      for (let i = 0; i < HEADER_TOGGLES.length; i++) {
        const name = HEADER_TOGGLES[i] as HeaderToggle;
        const r = toggleRect(vp, th, laneTop, laneH, i);
        const pressed =
          name === "lock" ? track.locked : name === "mute" ? track.muted : track.hidden;
        if (pressed) {
          ctx.fillStyle = th.marigoldSubtle;
          ctx.beginPath();
          ctx.roundRect(r.x, r.y, r.w, r.h, th.radiusSm);
          ctx.fill();
          this.stats.drawCalls++;
        }
        this.glyph(
          ctx,
          TOGGLE_ICON[name],
          r.x + r.w / 2,
          r.y + r.h / 2,
          th.iconSm,
          pressed ? th.marigoldText : th.textSecondary,
        );
      }
    }
  }

  // ---- ruler ------------------------------------------------------------------------------

  private drawRuler(
    ctx: CanvasRenderingContext2D,
    f: FrameInput,
    rx0: number,
    rx1: number,
    k: number,
  ): void {
    const { vp, theme: th, view } = f;
    const h = vp.rulerHeight;
    ctx.fillStyle = th.surface1;
    ctx.fillRect(0, 0, vp.width, h);
    ctx.fillStyle = th.borderSubtle;
    ctx.fillRect(0, h - th.borderThin, vp.width, th.borderThin);
    this.stats.drawCalls += 2;
    const px = (t: number): number => vp.headerWidth + (t - vp.scrollX) * k;

    if (view.range) {
      const a = Math.round(px(view.range.in));
      const b = Math.round(px(view.range.out));
      ctx.fillStyle = th.marigoldSubtle;
      ctx.fillRect(a, 0, b - a, h - th.borderThin);
      ctx.fillStyle = th.marigold;
      ctx.fillRect(a, 0, th.borderThin, h - th.borderThin);
      ctx.fillRect(b - th.borderThin, 0, th.borderThin, h - th.borderThin);
      this.stats.drawCalls += 3;
    }

    // The playhead flag (a pill with its timecode) is measured first: labels under it are skipped.
    const phx = px(view.playhead);
    const label = formatTimecode(view.playhead, f.fps, f.dropFrame);
    ctx.font = th.fontNumeric;
    const fw = ctx.measureText(label).width + 2 * th.space2;
    this.flagHalfWidth = fw / 2;
    const flagShown = phx + fw / 2 >= vp.headerWidth && phx - fw / 2 <= vp.width;
    const fx = flagX(phx, fw, vp);

    const plan = chooseTickPlan(vp.pxPerSec, f.fps, this.timecodeWidth + th.space4);
    const from = Math.max(rx0, vp.headerWidth);
    const t0 = xToTime(vp, from);
    const t1 = xToTime(vp, rx1);
    const first = Math.floor(t0 / plan.step) * plan.step - plan.step;
    const baseY = h - th.borderThin;
    ctx.beginPath();
    for (let t = first; t <= t1 + plan.step; t += plan.step) {
      const x = Math.round(px(t)) + 0.5;
      if (x >= vp.headerWidth) {
        ctx.moveTo(x, baseY);
        ctx.lineTo(x, baseY - th.space2);
      }
      for (let d = 1; d < plan.divisions; d++) {
        const mx = Math.round(px(t + (plan.step * d) / plan.divisions)) + 0.5;
        if (mx < vp.headerWidth || mx > rx1 + 1) continue;
        ctx.moveTo(mx, baseY);
        ctx.lineTo(mx, baseY - th.space1_5);
      }
    }
    ctx.strokeStyle = th.textMuted;
    ctx.lineWidth = th.borderThin;
    ctx.stroke();
    this.stats.drawCalls++;

    ctx.font = th.fontNumeric;
    ctx.fillStyle = th.textMuted;
    ctx.textBaseline = "middle";
    ctx.textAlign = "left";
    for (let t = first; t <= t1 + plan.step; t += plan.step) {
      if (t < 0) continue;
      const x = Math.round(px(t)) + 0.5;
      if (x < vp.headerWidth || x > rx1 + this.timecodeWidth) continue;
      const lx = x + th.space1;
      if (flagShown && lx < fx + fw + th.space1 && lx + this.timecodeWidth > fx - th.space1) {
        continue; // hidden under the playhead flag (issue #97)
      }
      ctx.fillText(formatTimecode(t, f.fps, f.dropFrame), lx, th.space0_5 + th.numericPx / 2 + 1);
      this.stats.drawCalls++;
    }

    // Corner above the header column.
    ctx.fillStyle = th.surface1;
    ctx.fillRect(0, 0, vp.headerWidth, h);
    ctx.fillStyle = th.borderSubtle;
    ctx.fillRect(0, h - th.borderThin, vp.headerWidth, th.borderThin);
    this.stats.drawCalls += 2;

    // Playhead line through the ruler, then the flag: pill with the timecode, kept inside the lane area.
    if (phx >= vp.headerWidth - 1 && phx <= vp.width + 1) {
      ctx.fillStyle = th.playhead;
      ctx.fillRect(Math.round(phx) - th.space0_5 / 2, 0, th.space0_5, h);
      this.stats.drawCalls++;
    }
    if (flagShown) {
      ctx.fillStyle = th.playhead;
      ctx.beginPath();
      ctx.roundRect(fx, 0, fw, th.space4, th.space4 / 2);
      ctx.fill();
      ctx.fillStyle = th.onMarigold;
      ctx.textAlign = "center";
      ctx.fillText(label, fx + fw / 2, th.space4 / 2 + 1);
      ctx.textAlign = "left";
      this.stats.drawCalls += 2;
    }
  }
}

/** Left edge of a playhead flag `fw` wide at canvas x `phx`: centered, kept inside the lane area. */
function flagX(phx: number, fw: number, vp: Viewport): number {
  return Math.min(Math.max(phx - fw / 2, vp.headerWidth), Math.max(vp.headerWidth, vp.width - fw));
}

function intersect(a: Rect, b: Rect): Rect {
  const x0 = Math.max(a.x, b.x);
  const y0 = Math.max(a.y, b.y);
  const x1 = Math.min(a.x + a.w, b.x + b.w);
  const y1 = Math.min(a.y + a.h, b.y + b.h);
  return { x: x0, y: y0, w: Math.max(0, x1 - x0), h: Math.max(0, y1 - y0) };
}
