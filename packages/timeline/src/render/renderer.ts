// SPDX-License-Identifier: GPL-3.0-or-later
// Virtualized Canvas2D renderer (arch §16, WP-2A.09). Draws only what intersects the (dirty)
// region:
//  - per track, the interval index yields just the visible clips; tracks outside the viewport are
//    skipped; nothing is ever iterated globally (`stats.visibleItems` counts what was drawn);
//  - draw calls are batched per track and clip kind (clips.ts);
//  - level of detail: labels, badges, tiles, waveforms and keyframes appear only when a clip is
//    wide enough (geometry.ts, shared with hit-testing); label count is capped per frame;
//  - filmstrips and waveforms come from LRU caches and never block a frame (media.ts);
//  - no per-frame allocation: buffers, rectangles, text and keys are reused (state.ts, text.ts).
// No literal colors: everything comes from the `Theme` resolved from the design tokens.
import { MAX_LABELS } from "../geometry.ts";
import type { Theme } from "../theme.ts";
import { FLICKS_PER_SECOND } from "../time.ts";
import type { Viewport } from "../viewport.ts";
import { drawTrack } from "./clips.ts";
import { drawHeaders } from "./headers.ts";
import { drawOverlays } from "./overlays.ts";
import { drawRuler, flagX } from "./ruler.ts";
import { type FrameInput, type FrameStats, RenderState } from "./state.ts";

export class TimelineRenderer {
  private readonly s = new RenderState();

  /** Draw counters of the last frame. */
  get stats(): FrameStats {
    return this.s.stats;
  }

  /** Half width of the playhead flag as last drawn; the hit-test uses it. */
  get flagHalfWidth(): number {
    return this.s.flagHalfWidth;
  }

  /**
   * Horizontal span (canvas CSS px) to repaint for a playhead at canvas x `phx`: the line, the
   * flag where it is drawn (kept inside the lane area) and every ruler label the flag hides, so
   * moving the playhead redraws those labels whole.
   */
  playheadSpan(phx: number, vp: Viewport, theme: Theme): { x0: number; x1: number } {
    const fw = 2 * (this.s.flagHalfWidth || 4 * theme.space4);
    const fx = flagX(phx, fw, vp);
    const reach = this.s.timecodeWidth + theme.space1 + 2;
    return { x0: Math.min(phx, fx) - reach, x1: Math.max(phx, fx + fw) + reach };
  }

  /** Forget cached text metrics (call after fonts load or the theme changes). */
  resetMetrics(): void {
    this.s.metricsFor = null;
  }

  draw(ctx: CanvasRenderingContext2D, f: FrameInput): void {
    const s = this.s;
    const { vp, theme: th, layout } = f;
    s.ctx = ctx;
    s.f = f;
    s.resetStats();
    s.labelsLeft = MAX_LABELS;
    s.prepare(ctx, th);
    s.k = vp.pxPerSec / FLICKS_PER_SECOND;

    ctx.setTransform(f.dpr, 0, 0, f.dpr, 0, 0);
    ctx.save();
    // The repainted region: the dirty rectangle clipped to the canvas, or all of it.
    const reg = s.region;
    if (f.region) {
      const x0 = Math.max(f.region.x, 0);
      const y0 = Math.max(f.region.y, 0);
      reg.x = x0;
      reg.y = y0;
      reg.w = Math.max(0, Math.min(f.region.x + f.region.w, vp.width) - x0);
      reg.h = Math.max(0, Math.min(f.region.y + f.region.h, vp.height) - y0);
    } else {
      reg.x = 0;
      reg.y = 0;
      reg.w = vp.width;
      reg.h = vp.height;
    }
    // Only a full frame asks for every visible tile, so only it may cancel loads that scrolled away.
    f.caches.beginFrame(reg.x <= 0 && reg.y <= 0 && reg.w >= vp.width && reg.h >= vp.height);
    if (reg.w <= 0 || reg.h <= 0) {
      ctx.restore();
      return;
    }
    ctx.beginPath();
    ctx.rect(reg.x, reg.y, reg.w, reg.h);
    ctx.clip();
    ctx.fillStyle = th.surface1;
    ctx.fillRect(reg.x, reg.y, reg.w, reg.h);
    s.stats.drawCalls += 2;

    const laneX0 = Math.max(reg.x, vp.headerWidth);
    const laneX1 = reg.x + reg.w;
    const laneY0 = Math.max(reg.y, vp.rulerHeight);
    const laneY1 = reg.y + reg.h;
    s.rx0 = laneX0;
    s.rx1 = laneX1;

    if (laneX1 > laneX0 && laneY1 > laneY0) {
      // One flick of margin so clips touching the edge are not culled by rounding.
      const t0 = Math.round(vp.scrollX + (laneX0 - vp.headerWidth) / s.k) - 1;
      const t1 = Math.round(vp.scrollX + (laneX1 - vp.headerWidth) / s.k) + 1;
      const n = layout.trackCount;
      for (let ti = 0; ti < n; ti++) {
        const laneTop = vp.rulerHeight + (layout.trackTop[ti] ?? 0) - vp.scrollY;
        const laneH = layout.trackHeight[ti] ?? 0;
        if (laneTop + laneH <= laneY0 || laneTop >= laneY1) continue; // vertical virtualization
        s.stats.tracksDrawn++;
        drawTrack(s, ti, laneTop, t0, t1);
      }
      drawOverlays(s);
    }

    if (reg.x < vp.headerWidth && laneY1 > laneY0) drawHeaders(s, laneY0, laneY1);
    if (reg.y < vp.rulerHeight) drawRuler(s);
    // Header column rule.
    ctx.fillStyle = th.borderSubtle;
    ctx.fillRect(vp.headerWidth - th.borderThin, 0, th.borderThin, vp.height);
    s.stats.drawCalls++;
    ctx.restore();
  }
}
