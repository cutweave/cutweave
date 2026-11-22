// SPDX-License-Identifier: GPL-3.0-or-later
// Preview canvas backing store + live resize renegotiation (WP-0C.02, arch §10 step 1).
//
// A ResizeObserver on the canvas ('device-pixel-content-box') gives its exact size in device
// pixels. On every change, in the same rendering step (ResizeObserver callbacks run after
// layout and before paint), the stage:
//   1. sets canvas.width/height to that size (this clears a canvas),
//   2. immediately redraws the last frame, scaled, so the cleared canvas is never composited
//      (no flicker), and
//   3. asks the engine to render at that size from now on (latest-wins, one message per
//      change). Frames already in flight at the old size are scaled until the new size lands.
// `mode` can pin the engine size instead (benchmarks comparable with WP-0C.01's 1080p runs).

import { FLAG_LOWRES, Samples } from "./loop.ts";
import type { Frame } from "./types.ts";

/** Engine output size: follow the canvas, or a fixed size (scaled to the canvas). */
export type EngineSizeMode = "canvas" | { width: number; height: number };

/** Engine defaults (`--max-width` / `--max-height`); the ring is sized for these. */
export const ENGINE_MAX_W = 2560;
export const ENGINE_MAX_H = 1440;

/** TS mirror of the engine's `fit_size` (engine/src/gpu.rs). */
export function fitSize(
  width: number,
  height: number,
  maxW = ENGINE_MAX_W,
  maxH = ENGINE_MAX_H,
): [number, number] {
  const w0 = Math.max(width, 1);
  const h0 = Math.max(height, 1);
  const s = Math.min(1, maxW / w0, maxH / h0);
  const w = Math.min(Math.max(Math.floor(w0 * s), 64), maxW);
  const h = Math.min(Math.max(Math.floor(h0 * s), 36), maxH);
  return [w & ~3, h & ~1];
}

export interface StageInfo {
  /** Canvas backing store (device pixels). */
  canvasWidth: number;
  canvasHeight: number;
  dpr: number;
  mode: EngineSizeMode;
  /** Last size requested from the engine and the size it should produce for it. */
  requested: [number, number] | null;
  expected: [number, number] | null;
  /** Backing-store changes seen. */
  resizes: number;
  /** Resizes that had nothing to redraw (the cleared canvas would have been shown). */
  blankResizes: number;
  /** Full-resolution frames presented whose size differed from `expected` (scaled). */
  scaledFrames: number;
  /** Size request -> first full-resolution frame of the expected size presented. */
  renegotiateMs: { n: number; p50: number; p95: number; max: number };
  /** True when the last full-resolution frame maps 1:1 onto the canvas pixels. */
  pixelExact: boolean;
}

export class CanvasStage {
  private ro: ResizeObserver | null = null;
  private requested: [number, number] | null = null;
  private expected: [number, number] | null = null;
  private pendingSince = Number.NaN;
  private resizes = 0;
  private blankResizes = 0;
  private scaledFrames = 0;
  private pixelExact = false;
  private readonly renegotiate = new Samples();
  private recentRenegotiate: number[] = [];

  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly redraw: () => boolean,
    private readonly sendResize: (width: number, height: number) => void,
    private mode: EngineSizeMode = "canvas",
  ) {}

  start(): void {
    this.ro = new ResizeObserver((entries) => {
      const e = entries[entries.length - 1];
      if (!e) return;
      const dp = e.devicePixelContentBoxSize?.[0];
      const w = dp ? dp.inlineSize : Math.round(e.contentRect.width * devicePixelRatio);
      const h = dp ? dp.blockSize : Math.round(e.contentRect.height * devicePixelRatio);
      this.applySize(w, h);
    });
    try {
      this.ro.observe(this.canvas, { box: "device-pixel-content-box" });
    } catch {
      this.ro.observe(this.canvas); // older engines: content box * devicePixelRatio
    }
  }

  private applySize(w: number, h: number): void {
    if (w <= 0 || h <= 0) return;
    if (w !== this.canvas.width || h !== this.canvas.height) {
      this.canvas.width = w;
      this.canvas.height = h;
      this.resizes++;
      if (!this.redraw()) this.blankResizes++;
    }
    this.request();
  }

  /** Send the engine size for the current mode (also after an engine restart). */
  request(force = false): void {
    const [w, h] =
      this.mode === "canvas"
        ? [this.canvas.width, this.canvas.height]
        : [this.mode.width, this.mode.height];
    if (w <= 0 || h <= 0) return;
    if (!force && this.requested?.[0] === w && this.requested?.[1] === h) return;
    this.requested = [w, h];
    this.expected = fitSize(w, h);
    this.pendingSince = performance.now();
    this.sendResize(w, h);
  }

  setMode(mode: EngineSizeMode): void {
    this.mode = mode;
    this.request(true);
  }

  /** Loop hook: track when the renegotiated size reaches the screen. */
  framePresented(f: Frame, tSubmit: number): void {
    if ((f.flags & FLAG_LOWRES) !== 0) return;
    const exp = this.expected;
    const matches = exp !== null && f.width === exp[0] && f.height === exp[1];
    if (!matches) this.scaledFrames++;
    else if (Number.isFinite(this.pendingSince)) {
      const ms = tSubmit - this.pendingSince;
      this.renegotiate.push(ms);
      this.recentRenegotiate.push(Math.round(ms * 100) / 100);
      this.pendingSince = Number.NaN;
    }
    this.pixelExact = f.width === this.canvas.width && f.height === this.canvas.height;
  }

  takeRenegotiateSamples(): number[] {
    const out = this.recentRenegotiate;
    this.recentRenegotiate = [];
    return out;
  }

  resetStats(): void {
    this.resizes = 0;
    this.blankResizes = 0;
    this.scaledFrames = 0;
    this.renegotiate.reset();
    this.recentRenegotiate = [];
  }

  info(): StageInfo {
    const s = this.renegotiate.summary();
    return {
      canvasWidth: this.canvas.width,
      canvasHeight: this.canvas.height,
      dpr: devicePixelRatio,
      mode: this.mode,
      requested: this.requested,
      expected: this.expected,
      resizes: this.resizes,
      blankResizes: this.blankResizes,
      scaledFrames: this.scaledFrames,
      renegotiateMs: { n: s.n, p50: s.p50, p95: s.p95, max: s.max },
      pixelExact: this.pixelExact,
    };
  }
}
