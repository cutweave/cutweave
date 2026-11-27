// SPDX-License-Identifier: GPL-3.0-or-later
// Draws the open keyframe lanes of one track (WP-2C.05): a strip under the clip per animated
// property, its name, and a mark per key whose shape tells the interpolation. Virtualized like
// the clips: only lanes of items in the repainted time range are visited, only keys in it are
// walked, and keys closer than `KEY_COLLAPSE_PX` collapse into one bar, so a lane never draws
// more marks than a third of its width in pixels. Strips that touch are one rectangle, and marks
// are batched per shape and state: at most ten fills per track however many keys it shows. No
// allocation per frame (`KeyMarks` is reused).
import type { RenderState } from "../render/state.ts";
import { anySelected, KEY_COLLAPSE_PX, type KeyframeLane, keyRadius, lowerBound } from "./lanes.ts";
import { BUCKETS, CLUSTER } from "./marks.ts";

/** Collects the marks of `lane` between canvas x `xa` and `xb` on the row centered at `cy`. */
function collectLane(s: RenderState, lane: KeyframeLane, xa: number, xb: number, cy: number): void {
  const { f, k } = s;
  const vp = f.vp;
  const times = lane.times;
  const n = times.length;
  if (n === 0) return;
  const reach = keyRadius(f.theme);
  const ta = vp.scrollX + (xa - reach - vp.headerWidth) / k;
  const tb = vp.scrollX + (xb + reach - vp.headerWidth) / k;
  const gap = KEY_COLLAPSE_PX / k;
  let j = lowerBound(times, ta);
  while (j < n) {
    const t = times[j] as number;
    if (t > tb) break;
    const x = vp.headerWidth + (t - vp.scrollX) * k;
    // Level of detail: the keys within the collapse distance of this one are one bar.
    let next = j + 1;
    if (next < n && (times[next] as number) - t < gap) next = lowerBound(times, t + gap, next);
    const selected = anySelected(lane.selected, j, next) ? 1 : 0;
    const glyph = next - j > 1 ? CLUSTER : Math.min(CLUSTER - 1, (lane.glyphs[j] as number) | 0);
    s.keys.push(x, cy, glyph * 2 + selected);
    j = next;
  }
}

function tracePath(s: RenderState, bucket: number, r: number): void {
  const { ctx, keys } = s;
  const glyph = bucket >> 1;
  ctx.beginPath();
  for (let i = 0; i < keys.length; i++) {
    if (keys.bucket[i] !== bucket) continue;
    const x = keys.x[i] as number;
    const y = keys.y[i] as number;
    switch (glyph) {
      case 0: // linear: a diamond
        ctx.moveTo(x, y - r);
        ctx.lineTo(x + r, y);
        ctx.lineTo(x, y + r);
        ctx.lineTo(x - r, y);
        ctx.closePath();
        break;
      case 1: // hold: a square
        ctx.rect(x - r + 1, y - r + 1, 2 * r - 2, 2 * r - 2);
        break;
      case 2: // bezier: a circle
        ctx.moveTo(x + r, y);
        ctx.arc(x, y, r, 0, Math.PI * 2);
        break;
      case 3: // spring: a triangle pointing along time
        ctx.moveTo(x - r + 1, y - r);
        ctx.lineTo(x + r, y);
        ctx.lineTo(x - r + 1, y + r);
        ctx.closePath();
        break;
      default: // a collapsed run: a bar
        ctx.rect(x - 1, y - r, KEY_COLLAPSE_PX, 2 * r);
    }
  }
}

/** Index in `open` (item indices by start) of the first item that can reach time `t`. */
function firstOpenFrom(s: RenderState, open: readonly number[], t: number): number {
  const { start, end } = s.f.layout;
  let lo = 0;
  let hi = open.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if ((start[open[mid] as number] ?? 0) < t) lo = mid + 1;
    else hi = mid;
  }
  // Items before it that still reach `t` (clips of a track rarely overlap: a step or two).
  while (lo > 0 && (end[open[lo - 1] as number] ?? 0) > t) lo--;
  return lo;
}

/**
 * Draws the open lanes of track `ti` whose lane top is at canvas y `laneTop`. Called after the
 * track's clips, inside the frame's clip region. Three passes over the items in the repainted
 * time range only: the strips (merged where they touch), the rules between rows in one fill,
 * then the names and the marks (one fill per shape and state).
 */
export function drawKeyframeLanes(s: RenderState, ti: number, laneTop: number): void {
  const { ctx, f, rx0, rx1, k } = s;
  const { layout, vp, theme: th } = f;
  const open = layout.laneItems[ti];
  if (open === undefined || open.length === 0) return;
  const row = layout.laneRow;
  const top = laneTop + layout.pad + layout.clipHeight(ti);
  const left = Math.max(rx0, vp.headerWidth);
  if (rx1 <= left) return;
  const r = keyRadius(th);
  const ta = vp.scrollX + (left - vp.headerWidth) / k;
  const tb = vp.scrollX + (rx1 - vp.headerWidth) / k;
  const from = firstOpenFrom(s, open, ta);

  // The strips: an inset band per item for its rows, on whole pixels. Strips that touch (or
  // are less than a pixel apart, as most are when zoomed out) are one rectangle, so a track of
  // two thousand clips in view fills a handful of rectangles, not two thousand slivers.
  let any = false;
  let rules = false;
  let runX0 = 0;
  let runX1 = Number.NEGATIVE_INFINITY;
  let runRows = 0;
  ctx.fillStyle = th.surface0;
  for (let n = from; n < open.length; n++) {
    const i = open[n] as number;
    if ((layout.start[i] ?? 0) > tb) break;
    const x0 = Math.round(Math.max(left, s.x(layout.start[i] ?? 0)));
    const x1 = Math.round(Math.min(rx1, s.x(layout.end[i] ?? 0)));
    const count = layout.lanesByItem.get(i)?.length ?? 0;
    if (x1 < x0 || count === 0) continue;
    any = true;
    if (count > 1) rules = true;
    if (count === runRows && x0 - runX1 <= 1) {
      runX1 = Math.max(runX1, x1);
      continue;
    }
    if (runRows > 0) {
      ctx.fillRect(runX0, top, Math.max(1, runX1 - runX0), runRows * row);
      s.stats.drawCalls++;
    }
    runX0 = x0;
    runX1 = x1;
    runRows = count;
  }
  if (!any) return;
  ctx.fillRect(runX0, top, Math.max(1, runX1 - runX0), runRows * row);
  s.stats.drawCalls++;

  // A rule between the rows of an item with several lanes.
  if (rules) {
    ctx.beginPath();
    for (let n = from; n < open.length; n++) {
      const i = open[n] as number;
      if ((layout.start[i] ?? 0) > tb) break;
      const x0 = Math.max(left, s.x(layout.start[i] ?? 0));
      const x1 = Math.min(rx1, s.x(layout.end[i] ?? 0));
      const count = layout.lanesByItem.get(i)?.length ?? 0;
      if (x1 <= x0) continue;
      for (let l = 1; l < count; l++) ctx.rect(x0, top + l * row, x1 - x0, th.borderThin);
    }
    ctx.fillStyle = th.borderSubtle;
    ctx.fill();
    s.stats.drawCalls++;
  }

  // The names (when all of one fits: no cut string to allocate) and the marks.
  const keys = s.keys;
  keys.clear();
  ctx.font = th.fontCaption;
  ctx.textBaseline = "middle";
  ctx.textAlign = "left";
  ctx.fillStyle = th.textMuted;
  for (let n = from; n < open.length; n++) {
    const i = open[n] as number;
    if ((layout.start[i] ?? 0) > tb) break;
    const x0 = Math.max(left, s.x(layout.start[i] ?? 0));
    const x1 = Math.min(rx1, s.x(layout.end[i] ?? 0));
    const lanes = layout.lanesByItem.get(i);
    if (x1 <= x0 || lanes === undefined) continue;
    const room = x1 - x0 - 2 * th.space1_5;
    for (let l = 0; l < lanes.length; l++) {
      const lane = lanes[l] as KeyframeLane;
      const cy = top + l * row + row / 2;
      if (s.labelsLeft > 0 && lane.label.length * s.charCaption <= room) {
        ctx.fillText(lane.label, x0 + th.space1_5, cy);
        s.labelsLeft--;
        s.stats.labels++;
        s.stats.drawCalls++;
      }
      collectLane(s, lane, x0, x1, cy);
    }
  }
  if (keys.length === 0) return;
  s.stats.visibleKeyframes += keys.length;
  for (let b = 0; b < BUCKETS; b++) {
    if ((keys.counts[b] ?? 0) === 0) continue;
    tracePath(s, b, r);
    ctx.fillStyle = b & 1 ? th.marigold : th.textPrimary;
    ctx.fill();
    s.stats.drawCalls++;
  }
}
