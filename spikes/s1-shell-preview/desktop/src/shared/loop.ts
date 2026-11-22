// SPDX-License-Identifier: GPL-3.0-or-later
// rAF present loop + instrumentation, shared by the two spike draw modes:
// - draw=main:    runs in the renderer main world; frames cross contextBridge (a copy);
// - draw=preload: runs in the preload's isolated world; pixels never cross contextBridge,
//                 only the stats snapshot does.

import { badRowStamps, sampledFnv1a } from "./integrity.ts";
import type { Frame, FramePolicy } from "./types.ts";

export interface PresentTarget {
  readonly kind: string;
  readonly adapter: string;
  draw(frame: Frame): void;
}

export class Samples {
  private v: number[] = [];
  push(x: number): void {
    if (Number.isFinite(x)) this.v.push(x);
  }
  reset(): void {
    this.v = [];
  }
  static pct(v: number[], p: number): number {
    if (v.length === 0) return Number.NaN;
    const s = [...v].sort((a, b) => a - b);
    return s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))] ?? Number.NaN;
  }
  summary(lastN?: number): LatSummary {
    const v = lastN === undefined ? this.v : this.v.slice(-lastN);
    const r = (x: number) => Math.round(x * 1000) / 1000;
    let max = Number.NaN;
    let sum = 0;
    for (const x of v) {
      sum += x;
      if (!(x <= max)) max = x;
    }
    return {
      n: v.length,
      p50: r(Samples.pct(v, 50)),
      p95: r(Samples.pct(v, 95)),
      p99: r(Samples.pct(v, 99)),
      max: r(max),
      mean: r(sum / (v.length || 1)),
    };
  }
}

export interface LatSummary {
  n: number;
  p50: number;
  p95: number;
  p99: number;
  max: number;
  mean: number;
}

export const LAT_KEYS = [
  "renderToPresentMs", // engine GPU submit -> renderer queue.submit (excludes compositor/scanout)
  "publishToPresentMs", // ring publish -> renderer queue.submit
  "publishToFetchMs", // ring publish -> addon copy done
  "addonCopyMs", // shm -> JS-heap ArrayBuffer (napi, seqlock-validated)
  "getFrameMs", // whole getFrame() call (draw=main: includes the contextBridge clone)
  "cloneMs", // getFrameMs - addonCopyMs (approx. contextBridge structured-clone cost)
  "verifyMs", // checksum + row stamps in JS
  "uploadDrawMs", // writeTexture + draw + submit (CPU side)
  "rafIntervalMs",
] as const;
export type LatKey = (typeof LAT_KEYS)[number];

export interface LoopSnapshot {
  seconds: number;
  drawIn: string;
  policy: FramePolicy;
  presenter: string;
  framesPresented: number;
  presentFps: number;
  /** Frames published by the engine that were never presented (seq gaps). */
  seqGaps: number;
  seqGapsPerMin: number;
  seqViolations: number;
  checksumFailures: number;
  rowStampFailures: number;
  longRafs: number;
  verify: boolean;
  format: number;
  size: string;
  lastPts: number;
  latency: Record<LatKey, LatSummary>;
}

export class PresentLoop {
  framesPresented = 0;
  framesSinceUp = 0;
  private seqGaps = 0;
  private seqViolations = 0;
  private checksumFailures = 0;
  private rowStampFailures = 0;
  private longRafs = 0;
  private startedAt = performance.now();
  private lastSeq = 0;
  private lastPts = 0;
  private lastFormat = 0;
  private lastSize = "";
  private lastRaf = 0;
  private running = false;
  private readonly lat = Object.fromEntries(LAT_KEYS.map((k) => [k, new Samples()])) as Record<
    LatKey,
    Samples
  >;

  constructor(
    private readonly drawIn: string,
    private readonly getFrame: (afterSeq: number, policy: FramePolicy) => Frame | null,
    private readonly nowUs: () => number,
    private readonly target: PresentTarget,
    public policy: FramePolicy,
    public verify: boolean,
  ) {}

  start(): void {
    if (this.running) return;
    this.running = true;
    requestAnimationFrame(this.tick);
  }

  /** New engine generation: seq restarts at 1. */
  engineRestarted(): void {
    this.lastSeq = 0;
    this.framesSinceUp = 0;
  }

  reset(): void {
    this.framesPresented = 0;
    this.seqGaps = 0;
    this.seqViolations = 0;
    this.checksumFailures = 0;
    this.rowStampFailures = 0;
    this.longRafs = 0;
    this.startedAt = performance.now();
    for (const k of LAT_KEYS) this.lat[k].reset();
  }

  private readonly tick = (now: number): void => {
    if (this.lastRaf) {
      const d = now - this.lastRaf;
      this.lat.rafIntervalMs.push(d);
      if (d > 25) this.longRafs++;
    }
    this.lastRaf = now;
    const t0 = performance.now();
    const f = this.getFrame(this.lastSeq, this.policy);
    const t1 = performance.now();
    if (f) {
      if (this.lastSeq > 0 && f.seq <= this.lastSeq) this.seqViolations++;
      if (this.lastSeq > 0 && f.seq > this.lastSeq + 1) this.seqGaps += f.seq - this.lastSeq - 1;
      let t2 = t1;
      if (this.verify) {
        const bytes = new Uint8Array(f.pixels);
        if (sampledFnv1a(bytes, f.payloadLen) !== f.checksum) this.checksumFailures++;
        if (badRowStamps(bytes, f.stride, f.height, f.seq) !== 0) this.rowStampFailures++;
        t2 = performance.now();
      }
      this.target.draw(f);
      const t3 = performance.now();
      const presentUs = this.nowUs();
      const L = this.lat;
      L.getFrameMs.push(t1 - t0);
      L.addonCopyMs.push(f.readUs / 1000);
      L.cloneMs.push(t1 - t0 - f.readUs / 1000);
      L.verifyMs.push(t2 - t1);
      L.uploadDrawMs.push(t3 - t2);
      L.publishToFetchMs.push((f.tFetchUs - f.tPublishUs) / 1000);
      L.publishToPresentMs.push((presentUs - f.tPublishUs) / 1000);
      L.renderToPresentMs.push((presentUs - f.tRenderUs) / 1000);
      this.framesPresented++;
      this.framesSinceUp++;
      this.lastSeq = f.seq;
      this.lastPts = f.pts;
      this.lastFormat = f.format;
      this.lastSize = `${f.width}x${f.height}`;
    }
    requestAnimationFrame(this.tick);
  };

  snapshot(recentN?: number): LoopSnapshot {
    const secs = (performance.now() - this.startedAt) / 1000;
    const r1 = (x: number) => Math.round(x * 10) / 10;
    return {
      seconds: r1(secs),
      drawIn: this.drawIn,
      policy: this.policy,
      presenter: `${this.target.kind} (${this.target.adapter})`,
      framesPresented: this.framesPresented,
      presentFps: Math.round((this.framesPresented / secs) * 100) / 100,
      seqGaps: this.seqGaps,
      seqGapsPerMin: r1((this.seqGaps / secs) * 60),
      seqViolations: this.seqViolations,
      checksumFailures: this.checksumFailures,
      rowStampFailures: this.rowStampFailures,
      longRafs: this.longRafs,
      verify: this.verify,
      format: this.lastFormat,
      size: this.lastSize,
      lastPts: this.lastPts,
      latency: Object.fromEntries(LAT_KEYS.map((k) => [k, this.lat[k].summary(recentN)])) as Record<
        LatKey,
        LatSummary
      >,
    };
  }
}
