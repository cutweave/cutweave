// SPDX-License-Identifier: GPL-3.0-or-later
// The ruler: adaptive ticks (never finer than a frame), time labels as timecode or frame numbers,
// the in/out band, sequence markers, and the playhead line and flag. Labels the flag would cover
// are skipped (issue #97).
import type { TimelineMarker } from "../model.ts";
import { chooseTickPlan } from "../ruler.ts";
import type { Viewport } from "../viewport.ts";
import type { RenderState } from "./state.ts";

/** Left edge of a playhead flag `fw` wide at canvas x `phx`: centered, kept inside the lane area. */
export function flagX(phx: number, fw: number, vp: Viewport): number {
  return Math.min(Math.max(phx - fw / 2, vp.headerWidth), Math.max(vp.headerWidth, vp.width - fw));
}

export function drawRuler(s: RenderState): void {
  const { ctx, f, rx0, rx1 } = s;
  const { vp, theme: th, view } = f;
  const format = f.display?.timeFormat ?? "timecode";
  const h = vp.rulerHeight;
  ctx.fillStyle = th.surface1;
  ctx.fillRect(0, 0, vp.width, h);
  ctx.fillStyle = th.borderSubtle;
  ctx.fillRect(0, h - th.borderThin, vp.width, th.borderThin);
  s.stats.drawCalls += 2;

  const range = view.range;
  if (range) {
    const a = Math.round(s.x(range.in));
    const b = Math.round(s.x(range.out));
    ctx.fillStyle = th.marigoldSubtle;
    ctx.fillRect(a, 0, b - a, h - th.borderThin);
    ctx.fillStyle = th.marigold;
    ctx.fillRect(a, 0, th.borderThin, h - th.borderThin);
    ctx.fillRect(b - th.borderThin, 0, th.borderThin, h - th.borderThin);
    s.stats.drawCalls += 3;
  }

  // The playhead flag (a pill with its time) is measured first: labels under it are skipped.
  const phx = s.x(view.playhead);
  const label = s.timeLabels.label(view.playhead, f.fps, f.dropFrame, format);
  ctx.font = th.fontNumeric;
  const fw = ctx.measureText(label).width + 2 * th.space2;
  s.flagHalfWidth = fw / 2;
  const flagShown = phx + fw / 2 >= vp.headerWidth && phx - fw / 2 <= vp.width;
  const fx = flagX(phx, fw, vp);

  const plan = chooseTickPlan(vp.pxPerSec, f.fps, s.timecodeWidth + th.space4, s.tickPlan);
  const from = Math.max(rx0, vp.headerWidth);
  const t0 = vp.scrollX + (from - vp.headerWidth) / s.k;
  const t1 = vp.scrollX + (rx1 - vp.headerWidth) / s.k;
  const first = Math.floor(t0 / plan.step) * plan.step - plan.step;
  const baseY = h - th.borderThin;
  ctx.beginPath();
  for (let t = first; t <= t1 + plan.step; t += plan.step) {
    const x = Math.round(s.x(t)) + 0.5;
    if (x >= vp.headerWidth) {
      ctx.moveTo(x, baseY);
      ctx.lineTo(x, baseY - th.space2);
    }
    for (let d = 1; d < plan.divisions; d++) {
      const mx = Math.round(s.x(t + (plan.step * d) / plan.divisions)) + 0.5;
      if (mx < vp.headerWidth || mx > rx1 + 1) continue;
      ctx.moveTo(mx, baseY);
      ctx.lineTo(mx, baseY - th.space1_5);
    }
  }
  ctx.strokeStyle = th.textMuted;
  ctx.lineWidth = th.borderThin;
  ctx.stroke();
  s.stats.drawCalls++;

  ctx.font = th.fontNumeric;
  ctx.fillStyle = th.textMuted;
  ctx.textBaseline = "middle";
  ctx.textAlign = "left";
  const labelY = th.space0_5 + th.numericPx / 2 + 1;
  for (let t = first; t <= t1 + plan.step; t += plan.step) {
    if (t < 0) continue;
    const x = Math.round(s.x(t)) + 0.5;
    if (x < vp.headerWidth || x > rx1 + s.timecodeWidth) continue;
    const lx = x + th.space1;
    if (flagShown && lx < fx + fw + th.space1 && lx + s.timecodeWidth > fx - th.space1) {
      continue; // hidden under the playhead flag (issue #97)
    }
    ctx.fillText(s.timeLabels.label(t, f.fps, f.dropFrame, format), lx, labelY);
    s.stats.drawCalls++;
  }

  drawMarkers(s, t0, t1, baseY);

  // Corner above the header column.
  ctx.fillStyle = th.surface1;
  ctx.fillRect(0, 0, vp.headerWidth, h);
  ctx.fillStyle = th.borderSubtle;
  ctx.fillRect(0, h - th.borderThin, vp.headerWidth, th.borderThin);
  s.stats.drawCalls += 2;

  // Playhead line through the ruler, then the flag: pill with the time, kept inside the lane area.
  if (phx >= vp.headerWidth - 1 && phx <= vp.width + 1) {
    ctx.fillStyle = th.playhead;
    ctx.fillRect(Math.round(phx) - th.space0_5 / 2, 0, th.space0_5, h);
    s.stats.drawCalls++;
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
    s.stats.drawCalls += 2;
  }
}

/** Height of a marker pin above the ruler's base line. */
export function markerPinHeight(th: { space1: number; space2: number }): number {
  return th.space2 + th.space1;
}

/**
 * Sequence markers in the lower part of the ruler, in their own colors (default marigold): a
 * range marker's span as a thin bar, a pin at every user and chapter marker (a chapter also gets
 * a stem up the ruler), a tick per beat (beats closer than 2 px are thinned). Consecutive markers
 * of one color share a path.
 */
function drawMarkers(s: RenderState, t0: number, t1: number, baseY: number): void {
  const { ctx, f } = s;
  const { layout, vp, theme: th } = f;
  const markers = layout.markers;
  if (markers.length === 0) return;
  const w = th.space2;
  const pinH = markerPinHeight(th);
  const r = th.radiusXs;
  const begin = layout.firstMarkerFrom(t0);

  // Pass 1: range bars (behind the pins, at the disabled opacity so ticks show through).
  let color: string | null = null;
  ctx.globalAlpha = th.opacityDisabled;
  for (let n = begin; n < markers.length; n++) {
    const m = markers[n] as TimelineMarker;
    if (m.t > t1) break;
    const d = m.duration ?? 0;
    if (d <= 0 || m.t + d < t0) continue;
    color = batch(s, color, m.color ?? th.marigold);
    const a = Math.max(Math.round(s.x(m.t)), vp.headerWidth);
    const b = Math.round(s.x(m.t + d));
    if (b > a) ctx.rect(a, baseY - th.space1, b - a, th.space1);
  }
  if (color !== null) flush(s);
  ctx.globalAlpha = 1;

  // Pass 2: pins, chapter stems and beat ticks.
  color = null;
  let lastBeat = Number.NEGATIVE_INFINITY;
  for (let n = begin; n < markers.length; n++) {
    const m = markers[n] as TimelineMarker;
    if (m.t > t1) break;
    if (m.t < t0 - w / 2 / s.k) continue;
    const x = Math.round(s.x(m.t)) + 0.5;
    if (x < vp.headerWidth) continue;
    color = batch(s, color, m.color ?? th.marigold);
    if (m.kind === "beat") {
      if (x - lastBeat < 2) continue;
      lastBeat = x;
      ctx.rect(x - th.borderThin / 2, baseY - th.space2, th.borderThin, th.space2);
    } else {
      // The pin: a small rounded body with a point at the base line, on the marker's time.
      ctx.roundRect(x - w / 2, baseY - pinH, w, pinH - w / 2 + r, r);
      ctx.moveTo(x - w / 2, baseY - w / 2);
      ctx.lineTo(x + w / 2, baseY - w / 2);
      ctx.lineTo(x, baseY);
      ctx.closePath();
      if (m.kind === "chapter") ctx.rect(x - th.borderThin / 2, 0, th.borderThin, baseY - pinH);
    }
    s.stats.markers++;
  }
  if (color !== null) flush(s);
}

/** Starts a new fill batch when `next` differs from the current color; returns the color. */
function batch(s: RenderState, current: string | null, next: string): string {
  if (current === next) return current;
  if (current !== null) flush(s);
  s.ctx.beginPath();
  s.ctx.fillStyle = next;
  return next;
}

function flush(s: RenderState): void {
  s.ctx.fill();
  s.stats.drawCalls++;
}
