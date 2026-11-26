// SPDX-License-Identifier: GPL-3.0-or-later
// Lanes and the clips on them. Per track the interval index yields just the visible clips; they
// are partitioned by (state group, kind) so each partition is one batch: one path for all bodies,
// one for the header strips, one for the linked badges, one for keyframes, one for waveform bars,
// one for selection outlines. Level of detail (geometry.ts, shared with hit-testing): labels,
// badges, tiles, waveforms and keyframes appear only when a clip is wide (and tall) enough.
import {
  KEYFRAME_MIN_PX,
  keyframeRowY,
  LABEL_MIN_PX,
  labelBand,
  showsKeyframes,
} from "../geometry.ts";
import {
  FLAG_DISABLED,
  FLAG_PROPOSED,
  FLAG_SELECTED,
  TRACK_DIMMED,
  TRACK_LOCKED,
  type TimelineLayout,
} from "../layout.ts";
import type { ItemKind } from "../model.ts";
import type { Viewport } from "../viewport.ts";
import { drawTiles, drawWaveforms } from "./media.ts";
import type { RenderState } from "./state.ts";

const KIND_INDEX: Readonly<Record<ItemKind, number>> = {
  media: 0,
  audio: 1,
  graphics: 2,
  captions: 3,
  effects: 4,
};
const KINDS: readonly ItemKind[] = ["media", "audio", "graphics", "captions", "effects"];

/** Left (`side` 0) or right (1) edge x of item `i`, rounded to a whole CSS pixel. */
export function edgeX(
  layout: TimelineLayout,
  vp: Viewport,
  i: number,
  k: number,
  side: 0 | 1,
): number {
  const left = Math.round(vp.headerWidth + ((layout.start[i] ?? 0) - vp.scrollX) * k);
  if (side === 0) return left;
  const x = Math.round(vp.headerWidth + ((layout.end[i] ?? 0) - vp.scrollX) * k);
  return Math.max(x, left + 1); // at least one pixel wide
}

/** The lane background of track `ti` and its visible clips, in `[t0, t1]` flicks. */
export function drawTrack(s: RenderState, ti: number, laneTop: number, t0: number, t1: number): void {
  const { ctx, f } = s;
  const { layout, vp, theme: th, view } = f;
  const stats = s.stats;
  const laneH = layout.trackHeight[ti] ?? 0;
  const laneColor = ti % 2 === 0 ? th.lane : th.laneAlt;
  ctx.fillStyle = laneColor;
  ctx.fillRect(vp.headerWidth, laneTop, vp.width - vp.headerWidth, laneH);
  stats.drawCalls++;

  const trackFlags = layout.trackFlags[ti] ?? 0;
  const ids = layout.query(ti, t0, t1, s.ids);
  if (ids.length > 0) {
    stats.visibleItems += ids.length;
    const cy = laneTop + layout.pad;
    const ch = layout.clipHeight(ti);

    // Partition by (state group, kind) so every bucket is one batch.
    const buckets = s.buckets;
    for (let b = 0; b < buckets.length; b++) (buckets[b] as number[]).length = 0;
    s.selected.length = 0;
    const sel = view.selection;
    for (let n = 0; n < ids.length; n++) {
      const i = ids[n] as number;
      const fl = layout.flags[i] ?? 0;
      const item = layout.items[i];
      if (item === undefined) continue;
      const group = fl & FLAG_PROPOSED ? 2 : fl & FLAG_DISABLED ? 1 : 0;
      (buckets[group * 5 + KIND_INDEX[item.kind]] as number[]).push(i);
      if (fl & FLAG_SELECTED || (sel.size > 0 && sel.has(item.id))) s.selected.push(i);
    }

    // A muted or hidden track dims its clips like disabled ones (the header shows which).
    const dimmed = (trackFlags & TRACK_DIMMED) !== 0;
    for (let group = 0; group < 3; group++) {
      ctx.globalAlpha = group === 1 || dimmed ? th.opacityDisabled : 1;
      for (let kind = 0; kind < 5; kind++) {
        const bucket = buckets[group * 5 + kind] as number[];
        if (bucket.length > 0) drawBucket(s, bucket, group, KINDS[kind] as ItemKind, cy, ch);
      }
    }
    ctx.globalAlpha = 1;

    if (showsKeyframes(ch, th)) drawKeyframes(s, ids, cy, ch);
  }

  if (trackFlags & TRACK_LOCKED) drawLocked(s, laneTop, laneH, laneColor);
  if (ids.length > 0) drawSelection(s, ti, laneTop + layout.pad, layout.clipHeight(ti));
}

/** A locked lane: its color at half strength over the clips, and a diagonal hatch. */
function drawLocked(s: RenderState, laneTop: number, laneH: number, color: string): void {
  const { ctx, f, rx0, rx1 } = s;
  const { vp, theme: th } = f;
  ctx.globalAlpha = th.opacityDisabled;
  ctx.fillStyle = color;
  ctx.fillRect(vp.headerWidth, laneTop, vp.width - vp.headerWidth, laneH);
  ctx.globalAlpha = 1;
  const pitch = 2 * th.space2;
  ctx.save();
  ctx.beginPath();
  ctx.rect(vp.headerWidth, laneTop, vp.width - vp.headerWidth, laneH);
  ctx.clip();
  ctx.beginPath();
  // Anchored to lane time zero's pixel grid modulo the pitch, so the hatch does not crawl on pan.
  const phase = (((vp.scrollX * s.k) % pitch) + pitch) % pitch;
  for (let hx = Math.floor(rx0 / pitch) * pitch - laneH - phase; hx < rx1 + pitch; hx += pitch) {
    ctx.moveTo(hx, laneTop + laneH);
    ctx.lineTo(hx + laneH, laneTop);
  }
  ctx.strokeStyle = th.borderSubtle;
  ctx.lineWidth = th.borderThin;
  ctx.stroke();
  ctx.restore();
  s.stats.drawCalls += 3;
}

function drawBucket(
  s: RenderState,
  bucket: readonly number[],
  group: number,
  kind: ItemKind,
  cy: number,
  ch: number,
): void {
  const { ctx, f, k, rx0, rx1 } = s;
  const { layout, vp, theme: th } = f;
  const stats = s.stats;
  const col = th.clip[kind];
  const proposed = group === 2;
  const r = th.radiusSm;
  const bw = th.borderThin;

  // Bodies: one rounded rect per clip in one path (fill, then 1px border stroke).
  ctx.beginPath();
  for (let n = 0; n < bucket.length; n++) {
    const i = bucket[n] as number;
    const x = edgeX(layout, vp, i, k, 0);
    const w = edgeX(layout, vp, i, k, 1) - x;
    ctx.roundRect(x + bw / 2, cy + bw / 2, Math.max(1, w - bw), ch - bw, r);
  }
  ctx.fillStyle = proposed ? th.agentSubtle : col.body;
  ctx.fill();
  ctx.strokeStyle = proposed ? th.agent : col.strong;
  ctx.lineWidth = bw;
  if (proposed) ctx.setLineDash(s.dashProposal);
  ctx.stroke();
  if (proposed) {
    ctx.setLineDash(NO_DASH);
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
    stats.drawCalls += 2;
  }

  // Header strips: 3px lines in the strong color, inset from the rounded corners.
  ctx.beginPath();
  for (let n = 0; n < bucket.length; n++) {
    const i = bucket[n] as number;
    const x = edgeX(layout, vp, i, k, 0);
    const w = edgeX(layout, vp, i, k, 1) - x;
    if (w < 2 * r + 1) continue;
    ctx.moveTo(x + r / 2, cy + th.clipStrip / 2);
    ctx.lineTo(x + w - r / 2, cy + th.clipStrip / 2);
  }
  ctx.strokeStyle = proposed ? th.agent : col.strong;
  ctx.lineWidth = th.clipStrip;
  ctx.lineCap = "butt";
  ctx.stroke();
  stats.drawCalls += 4;

  // Body area under the label band: tiles and waveform (a collapsed track has no room).
  const band = labelBand(th);
  const areaY = cy + th.clipStrip + band;
  const areaH = cy + ch - bw - areaY;
  if (areaH >= th.space4) {
    if (f.caches.hasFilmstripLoader) drawTiles(s, bucket, areaY, areaH);
    if (kind === "audio" && f.caches.hasWaveformLoader) {
      drawWaveforms(s, bucket, col.strong, areaY, areaH);
    }
  }

  drawLabels(s, bucket, cy, band);
}

/** Linked badge (a chain of two links) and the label in each clip's label band. */
function drawLabels(s: RenderState, bucket: readonly number[], cy: number, band: number): void {
  const { ctx, f, k } = s;
  const { layout, vp, theme: th } = f;
  const stats = s.stats;
  const midY = cy + th.clipStrip + band / 2;
  // The badge: two pills `space-2 + space-0-5` wide, `space-1-5` tall, `space-1-5` apart.
  const pillW = th.space2 + th.space0_5;
  const pillH = th.space1_5;
  const badgeW = th.space1_5 + pillW;

  let badges = false;
  ctx.beginPath();
  for (let n = 0; n < bucket.length; n++) {
    const i = bucket[n] as number;
    if (layout.items[i]?.linkedGroup === undefined) continue;
    const x = edgeX(layout, vp, i, k, 0);
    const w = edgeX(layout, vp, i, k, 1) - x;
    if (w < LABEL_MIN_PX) continue;
    const bx = Math.max(x, vp.headerWidth) + th.space2;
    if (bx + badgeW > x + w - th.space2) continue;
    ctx.roundRect(bx, midY - pillH / 2, pillW, pillH, pillH / 2);
    ctx.roundRect(bx + th.space1_5, midY - pillH / 2, pillW, pillH, pillH / 2);
    badges = true;
  }
  if (badges) {
    ctx.strokeStyle = th.textSecondary;
    ctx.lineWidth = th.borderThin + th.borderThin / 2;
    ctx.stroke();
    stats.drawCalls++;
  }

  ctx.fillStyle = th.textPrimary;
  ctx.font = th.fontCaption;
  ctx.textBaseline = "middle";
  ctx.textAlign = "left";
  for (let n = 0; n < bucket.length && s.labelsLeft > 0; n++) {
    const i = bucket[n] as number;
    const item = layout.items[i];
    if (item === undefined) continue;
    const x = edgeX(layout, vp, i, k, 0);
    const w = edgeX(layout, vp, i, k, 1) - x;
    if (w < LABEL_MIN_PX) continue;
    let lx = Math.max(x, vp.headerWidth) + th.space2;
    if (item.linkedGroup !== undefined && lx + badgeW <= x + w - th.space2) {
      lx += badgeW + th.space1;
    }
    const avail = x + w - th.space2 - lx;
    if (avail < th.space4) continue;
    const text = s.itemLabels.fit(layout, layout.items.length, i, item.label, avail, s.charCaption);
    if (text === "") continue;
    ctx.fillText(text, lx, midY);
    s.labelsLeft--;
    stats.labels++;
    stats.drawCalls++;
  }
}

function drawKeyframes(s: RenderState, ids: readonly number[], cy: number, ch: number): void {
  const { ctx, f, k, rx0, rx1 } = s;
  const { layout, vp, theme: th } = f;
  const rowY = keyframeRowY(th, cy, ch);
  const r = th.space1;
  const ta = vp.scrollX + (rx0 - vp.headerWidth) / k;
  const tb = vp.scrollX + (rx1 - vp.headerWidth) / k;
  let any = false;
  ctx.beginPath();
  for (let n = 0; n < ids.length; n++) {
    const i = ids[n] as number;
    const kfs = layout.items[i]?.keyframes;
    if (kfs === undefined || kfs.length === 0) continue;
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
      s.stats.visibleKeyframes++;
      any = true;
    }
  }
  if (any) {
    ctx.fillStyle = th.textPrimary;
    ctx.fill();
    s.stats.drawCalls++;
  }
}

/** Selection outlines (2px marigold, never a tint) and trim handles on selected or hovered clips. */
function drawSelection(s: RenderState, ti: number, cy: number, ch: number): void {
  const { ctx, f, k } = s;
  const { layout, vp, theme: th } = f;
  const selected = s.selected;
  const hover = f.hoverItem;
  const hoverHere = hover >= 0 && layout.trackOf[hover] === ti && !selected.includes(hover);
  if (selected.length === 0 && !hoverHere) return;
  ctx.beginPath();
  const o = th.borderThin;
  for (let n = 0; n < selected.length; n++) {
    const i = selected[n] as number;
    const x = edgeX(layout, vp, i, k, 0);
    const w = edgeX(layout, vp, i, k, 1) - x;
    ctx.roundRect(x + o, cy + o, Math.max(1, w - 2 * o), ch - 2 * o, Math.max(0, th.radiusSm - o));
  }
  ctx.strokeStyle = th.marigold;
  ctx.lineWidth = 2;
  ctx.stroke();
  s.stats.drawCalls++;
  // Trim handles: short marigold bars inside each edge.
  ctx.beginPath();
  for (let n = 0; n < selected.length; n++) handleBars(s, selected[n] as number, cy, ch);
  if (hoverHere) handleBars(s, hover, cy, ch);
  ctx.strokeStyle = th.marigold;
  ctx.lineWidth = th.space0_5 + 1;
  ctx.lineCap = "round";
  ctx.stroke();
  ctx.lineCap = "butt";
  s.stats.drawCalls++;
}

function handleBars(s: RenderState, i: number, cy: number, ch: number): void {
  const { ctx, f, k } = s;
  const { layout, vp, theme: th } = f;
  const x = edgeX(layout, vp, i, k, 0);
  const w = edgeX(layout, vp, i, k, 1) - x;
  if (w < 6 * th.space1) return;
  const y0 = cy + ch * 0.3;
  const y1 = cy + ch * 0.7;
  const inset = th.space1;
  ctx.moveTo(x + inset, y0);
  ctx.lineTo(x + inset, y1);
  ctx.moveTo(x + w - inset, y0);
  ctx.lineTo(x + w - inset, y1);
}

/** No dash (a solid line). */
export const NO_DASH: number[] = [];
