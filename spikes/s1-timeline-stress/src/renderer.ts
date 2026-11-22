// SPDX-License-Identifier: GPL-3.0-or-later
// Virtualized Canvas2D timeline renderer. Only items intersecting the viewport are
// visited (per-track interval index) and draw calls are batched per track.
import { type Kind, THUMB_COUNT } from "./model.ts";
import { THUMB_H, THUMB_W } from "./thumbs.ts";
import { LABEL_MIN_PX, THUMB_MIN_PX, type View } from "./view.ts";

/** Mirrors the `caption` type token (11px); canvas cannot read a composite font token. */
const LABEL_FONT_PX = 11;
const LABEL_BAND = LABEL_FONT_PX + 5;
const MAX_LABELS = 400;

export interface FrameCounters {
  drawCalls: number;
  visibleClips: number;
  visibleKeyframes: number;
  thumbTiles: number;
  labels: number;
}

export class Renderer {
  readonly counters: FrameCounters = {
    drawCalls: 0,
    visibleClips: 0,
    visibleKeyframes: 0,
    thumbTiles: 0,
    labels: 0,
  };
  private readonly ctx: CanvasRenderingContext2D;
  private dpr = 1;
  private readonly ids: number[] = [];
  private readonly sp = { x: 0, w: 0 };

  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly view: View,
    private readonly thumbs: readonly ImageBitmap[],
  ) {
    // alpha:false lets the compositor skip blending; desynchronized is left to the browser.
    const ctx = canvas.getContext("2d", { alpha: false });
    if (!ctx) throw new Error("Canvas2D unavailable");
    this.ctx = ctx;
  }

  resize(cssW: number, cssH: number, dpr: number): void {
    this.dpr = dpr;
    this.canvas.width = Math.max(1, Math.round(cssW * dpr));
    this.canvas.height = Math.max(1, Math.round(cssH * dpr));
    this.canvas.style.width = `${cssW}px`;
    this.canvas.style.height = `${cssH}px`;
    this.view.width = cssW;
    this.view.height = cssH;
    this.view.clamp();
  }

  draw(): void {
    const { ctx, view } = this;
    const th = view.theme;
    const c = this.counters;
    c.drawCalls = c.visibleClips = c.visibleKeyframes = c.thumbTiles = c.labels = 0;
    ctx.setTransform(this.dpr, 0, 0, this.dpr, 0, 0);
    const W = view.width;
    const H = view.height;

    ctx.fillStyle = th.surface1;
    ctx.fillRect(0, 0, W, H);
    c.drawCalls++;

    // Tracks (clipped to the area below the ruler).
    ctx.save();
    ctx.beginPath();
    ctx.rect(0, view.rulerH, W, H - view.rulerH);
    ctx.clip();
    const t0 = view.t0;
    const t1 = view.t1;
    const pps = view.pps;
    let labelsLeft = MAX_LABELS;
    ctx.font = `${LABEL_FONT_PX}px ${th.fontUi}`;
    ctx.textBaseline = "middle";

    for (let tr = 0; tr < view.trackTop.length; tr++) {
      const ty = view.trackScreenY(tr);
      const trH = view.trackH[tr] ?? 0;
      if (ty + trH < view.rulerH || ty > H) continue; // vertical virtualization

      ctx.fillStyle = tr % 2 === 0 ? th.lane : th.laneAlt;
      ctx.fillRect(0, ty, W, trH);
      c.drawCalls++;

      const kind: Kind = view.kindOf(tr);
      const col = th.clip[kind];
      const ids = view.tl.index[tr]?.query(t0, t1, this.ids) ?? this.ids;
      c.visibleClips += ids.length;
      const cy = ty + 1;
      const ch = trH - 2;

      // Bodies, then edge strips: one path + one fill each per track.
      ctx.beginPath();
      for (const id of ids) {
        const { x, w } = this.span(id);
        ctx.rect(x, cy, w, ch);
      }
      ctx.fillStyle = col.body;
      ctx.fill();
      ctx.beginPath();
      for (const id of ids) {
        const { x, w } = this.span(id);
        ctx.rect(x, cy, w, th.stripPx);
      }
      ctx.fillStyle = col.edge;
      ctx.fill();
      c.drawCalls += 2;

      // Thumbnail tiles (media tracks, wide-enough clips, visible x-range only).
      if (kind === "media") {
        const areaY = cy + th.stripPx + LABEL_BAND;
        const areaH = cy + ch - areaY;
        const tileW = (areaH * THUMB_W) / THUMB_H;
        for (const id of ids) {
          const { x, w } = this.span(id);
          if (w < THUMB_MIN_PX) continue;
          const vx0 = Math.max(x, 0);
          const vx1 = Math.min(x + w, W);
          for (let tx = x + Math.floor((vx0 - x) / tileW) * tileW; tx < vx1; tx += tileW) {
            const drawW = Math.min(tileW, x + w - tx);
            const bmp = this.thumbs[(id + Math.round((tx - x) / tileW)) % THUMB_COUNT];
            if (!bmp) continue;
            ctx.drawImage(bmp, 0, 0, (drawW / tileW) * THUMB_W, THUMB_H, tx, areaY, drawW, areaH);
            c.drawCalls++;
            c.thumbTiles++;
          }
        }
      }

      // Labels.
      ctx.fillStyle = th.textPrimary;
      for (const id of ids) {
        if (labelsLeft <= 0) break;
        const { x, w } = this.span(id);
        if (w < LABEL_MIN_PX) continue;
        ctx.fillText(
          `${kind} ${id}`,
          Math.max(x, 0) + th.space1,
          cy + th.stripPx + LABEL_BAND / 2,
          w - th.space2,
        );
        labelsLeft--;
        c.labels++;
        c.drawCalls++;
      }

      // Keyframe diamonds: one batched path per track; binary search inside each clip.
      const rowY = ty + trH - view.keyframeRowFromBottom;
      const r = th.space1;
      ctx.beginPath();
      let any = false;
      for (const id of ids) {
        if (!view.keyframesVisible(id)) continue;
        const off = view.tl.kfOff[id] ?? 0;
        const n = view.tl.kfCount[id] ?? 0;
        let lo = off;
        let hi = off + n;
        while (lo < hi) {
          const mid = (lo + hi) >>> 1;
          if ((view.tl.kfTime[mid] ?? 0) < t0) lo = mid + 1;
          else hi = mid;
        }
        for (let k = lo; k < off + n; k++) {
          const kt = view.tl.kfTime[k] ?? 0;
          if (kt > t1) break;
          if (k === view.selectedKeyframe) continue;
          const x = (kt - t0) * pps;
          ctx.moveTo(x, rowY - r);
          ctx.lineTo(x + r, rowY);
          ctx.lineTo(x, rowY + r);
          ctx.lineTo(x - r, rowY);
          ctx.closePath();
          c.visibleKeyframes++;
          any = true;
        }
      }
      if (any) {
        ctx.fillStyle = th.textPrimary;
        ctx.fill();
        c.drawCalls++;
      }

      // Selected clips: marigold outline (selection = "here").
      if (view.selectedClips.size > 0) {
        ctx.strokeStyle = th.marigold;
        ctx.lineWidth = 2;
        ctx.beginPath();
        let sel = false;
        for (const id of ids) {
          if (!view.selectedClips.has(id)) continue;
          const { x, w } = this.span(id);
          ctx.rect(x, cy, w, ch);
          sel = true;
        }
        if (sel) {
          ctx.stroke();
          c.drawCalls++;
        }
      }
    }

    // Selected keyframe highlight.
    const sk = view.selectedKeyframe;
    if (sk >= 0) {
      const kt = view.tl.kfTime[sk] ?? 0;
      let clip = -1;
      for (let tr = 0; tr < view.trackTop.length && clip < 0; tr++) {
        for (const id of view.tl.index[tr]?.query(kt, kt, this.ids) ?? []) {
          if (sk >= (view.tl.kfOff[id] ?? 0) && sk < (view.tl.kfOff[id] ?? 0) + (view.tl.kfCount[id] ?? 0)) {
            clip = id;
            break;
          }
        }
      }
      if (clip >= 0) {
        const tr = view.tl.track[clip] ?? 0;
        const y = view.trackScreenY(tr) + (view.trackH[tr] ?? 0) - view.keyframeRowFromBottom;
        const x = (kt - t0) * pps;
        const r = th.space1 + 1;
        ctx.fillStyle = th.marigold;
        ctx.beginPath();
        ctx.moveTo(x, y - r);
        ctx.lineTo(x + r, y);
        ctx.lineTo(x, y + r);
        ctx.lineTo(x - r, y);
        ctx.closePath();
        ctx.fill();
        c.drawCalls++;
      }
    }

    // Marquee.
    const m = view.marquee;
    if (m) {
      ctx.fillStyle = th.marigoldSubtle;
      ctx.strokeStyle = th.marigold;
      ctx.lineWidth = 1;
      ctx.fillRect(Math.min(m.x0, m.x1), Math.min(m.y0, m.y1), Math.abs(m.x1 - m.x0), Math.abs(m.y1 - m.y0));
      ctx.strokeRect(Math.min(m.x0, m.x1), Math.min(m.y0, m.y1), Math.abs(m.x1 - m.x0), Math.abs(m.y1 - m.y0));
      c.drawCalls += 2;
    }
    ctx.restore();

    this.drawRuler();
    this.drawPlayhead();
  }

  private span(id: number): { x: number; w: number } {
    const { view } = this;
    const x = (view.tl.start[id] ?? 0) - view.t0;
    const w = Math.max(1, ((view.tl.end[id] ?? 0) - (view.tl.start[id] ?? 0)) * view.pps - 1);
    // Reused result object: no per-clip allocation in the hot loop.
    this.sp.x = x * view.pps;
    this.sp.w = w;
    return this.sp;
  }

  private drawRuler(): void {
    const { ctx, view } = this;
    const th = view.theme;
    ctx.fillStyle = th.surface0;
    ctx.fillRect(0, 0, view.width, view.rulerH);
    const steps = [1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 1800, 3600];
    const minPx = view.rulerH * 3;
    const step = steps.find((s) => s * view.pps >= minPx) ?? 3600;
    const first = Math.floor(view.t0 / step) * step;
    ctx.strokeStyle = th.textMuted;
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.fillStyle = th.textMuted;
    ctx.font = `${LABEL_FONT_PX}px ${th.fontMono}`;
    ctx.textBaseline = "middle";
    for (let t = first; t <= view.t1; t += step) {
      const x = Math.round((t - view.t0) * view.pps) + 0.5;
      ctx.moveTo(x, view.rulerH - th.space2);
      ctx.lineTo(x, view.rulerH);
      const mm = Math.floor(t / 60);
      const ss = Math.floor(t % 60);
      ctx.fillText(`${mm}:${String(ss).padStart(2, "0")}`, x + th.space1, view.rulerH / 2 - 2);
      this.counters.drawCalls++;
    }
    ctx.stroke();
    this.counters.drawCalls += 2;
  }

  private drawPlayhead(): void {
    const { ctx, view } = this;
    const x = Math.round(view.timeToX(view.playhead)) + 0.5;
    if (x < -8 || x > view.width + 8) return;
    const th = view.theme;
    ctx.fillStyle = th.playhead;
    ctx.fillRect(x - 0.5, view.rulerH, 1, view.height - view.rulerH);
    const s = th.space2;
    ctx.beginPath();
    ctx.moveTo(x - s, view.rulerH - s);
    ctx.lineTo(x + s, view.rulerH - s);
    ctx.lineTo(x, view.rulerH);
    ctx.closePath();
    ctx.fill();
    this.counters.drawCalls += 2;
  }
}
