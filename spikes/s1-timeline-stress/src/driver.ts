// SPDX-License-Identifier: GPL-3.0-or-later
// Scripted pan/zoom sweep (the "interaction driver") and the metrics collector.
// The sweep calls the same View methods the wheel/drag handlers call.
import { percentile, round, sortedCopy } from "./stats.ts";
import { MAX_PPS, type View } from "./view.ts";
import type { Renderer } from "./renderer.ts";

const ZOOM_PERIOD_S = 12;
const PAN_PERIOD_S = 17;
const SCROLL_PERIOD_S = 9;
/** Zoom range of the sweep: whole timeline in view .. ~one clip fills 1/4 of the screen. */
const SWEEP_MIN_PPS = 0.5;
const SWEEP_MAX_PPS = Math.min(MAX_PPS, 300);

/** Positions the view as a pure function of elapsed seconds. */
export function applySweep(view: View, tSec: number): void {
  const z = 0.5 - 0.5 * Math.cos((2 * Math.PI * tSec) / ZOOM_PERIOD_S);
  view.pps = Math.exp(Math.log(SWEEP_MIN_PPS) + z * (Math.log(SWEEP_MAX_PPS) - Math.log(SWEEP_MIN_PPS)));
  const centre = view.tl.duration / 2 + (view.tl.duration / 2.2) * Math.sin((2 * Math.PI * tSec) / PAN_PERIOD_S);
  view.t0 = centre - view.width / view.pps / 2;
  view.y0 = view.maxY0 * (0.5 - 0.5 * Math.cos((2 * Math.PI * tSec) / SCROLL_PERIOD_S));
  view.clamp();
}

export interface BenchResult {
  mode: string;
  seconds: number;
  viewport: { width: number; height: number; dpr: number };
  dataset: { clips: number; keyframes: number; thumbnails: number; tracks: number };
  frames: number;
  fps: { mean: number; p50: number; p5: number };
  frameMs: { p50: number; p95: number; p99: number; max: number };
  drawMs: { p50: number; p95: number; p99: number; max: number };
  longFrames: { over20ms: number; over33ms: number; over50ms: number };
  visible: { clipsMax: number; clipsP50: number; keyframesMax: number; thumbTilesMax: number; drawCallsMax: number; drawCallsP50: number };
  hitTest: { calls: number; meanMs: number; p95Ms: number; p99Ms: number; maxMs: number; hitRate: number };
  memory: { jsHeapUsedMB: number | null; uaSpecificMB: number | null };
  userAgent: string;
}

export class Collector {
  frameMs: number[] = [];
  drawMs: number[] = [];
  clips: number[] = [];
  calls: number[] = [];
  kfMax = 0;
  thumbMax = 0;
  record(frame: number, draw: number, r: Renderer): void {
    this.frameMs.push(frame);
    this.drawMs.push(draw);
    this.clips.push(r.counters.visibleClips);
    this.calls.push(r.counters.drawCalls);
    this.kfMax = Math.max(this.kfMax, r.counters.visibleKeyframes);
    this.thumbMax = Math.max(this.thumbMax, r.counters.thumbTiles);
  }
}

/** Times single hitTest calls at random viewport points while the sweep is at a mid zoom. */
export function benchHitTest(view: View, n = 20000): BenchResult["hitTest"] {
  let seed = 12345;
  const rnd = () => {
    seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
    return seed / 4294967296;
  };
  const samples: number[] = [];
  let hits = 0;
  // Sample across zoom levels so the zoomed-out (worst) case is included.
  for (let i = 0; i < n; i++) {
    view.pps = Math.exp(Math.log(SWEEP_MIN_PPS) + rnd() * (Math.log(SWEEP_MAX_PPS) - Math.log(SWEEP_MIN_PPS)));
    view.t0 = rnd() * view.tl.duration - view.width / view.pps / 2;
    view.y0 = rnd() * view.maxY0;
    view.clamp();
    const x = rnd() * view.width;
    const y = view.rulerH + rnd() * (view.height - view.rulerH);
    const a = performance.now();
    const h = view.hitTest(x, y);
    samples.push(performance.now() - a);
    if (h) hits++;
  }
  const s = sortedCopy(samples);
  return {
    calls: n,
    meanMs: round(samples.reduce((p, q) => p + q, 0) / n, 4),
    p95Ms: round(percentile(s, 95), 4),
    p99Ms: round(percentile(s, 99), 4),
    maxMs: round(s[s.length - 1] ?? 0, 4),
    hitRate: round(hits / n, 3),
  };
}

export function summarize(
  c: Collector,
  base: Pick<BenchResult, "mode" | "seconds" | "viewport" | "dataset" | "hitTest" | "memory" | "userAgent">,
): BenchResult {
  // Drop the first frame (includes warmup of fonts/JIT) from rate statistics.
  const fm = c.frameMs.slice(1);
  const sf = sortedCopy(fm);
  const sd = sortedCopy(c.drawMs.slice(1));
  const total = fm.reduce((p, q) => p + q, 0);
  const sc = sortedCopy(c.clips);
  const scall = sortedCopy(c.calls);
  const q = (s: number[]) => ({
    p50: round(percentile(s, 50)),
    p95: round(percentile(s, 95)),
    p99: round(percentile(s, 99)),
    max: round(s[s.length - 1] ?? 0),
  });
  return {
    ...base,
    frames: fm.length,
    fps: {
      mean: round(1000 / (total / fm.length)),
      p50: round(1000 / percentile(sf, 50)),
      // p5 of the frame *rate* == rate at the p95 of frame time.
      p5: round(1000 / percentile(sf, 95)),
    },
    frameMs: q(sf),
    drawMs: q(sd),
    longFrames: {
      over20ms: fm.filter((x) => x > 20).length,
      over33ms: fm.filter((x) => x > 33.4).length,
      over50ms: fm.filter((x) => x > 50).length,
    },
    visible: {
      clipsMax: sc[sc.length - 1] ?? 0,
      clipsP50: percentile(sc, 50),
      keyframesMax: c.kfMax,
      thumbTilesMax: c.thumbMax,
      drawCallsMax: scall[scall.length - 1] ?? 0,
      drawCallsP50: percentile(scall, 50),
    },
  };
}
