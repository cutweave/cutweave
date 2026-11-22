// SPDX-License-Identifier: GPL-3.0-or-later
// rAF present loop + instrumentation, shared by the two spike draw modes:
// - draw=main:    runs in the renderer main world; frames cross contextBridge (a copy);
// - draw=preload: runs in the preload's isolated world; pixels never cross contextBridge,
//                 only the stats snapshot does.
//
// WP-0C.02 adds scrub-to-pixel tracking (`noteScrub`): for every seek request, the time from
// its input event to the start of the animation frame after the matching frame was submitted
// (when the compositor picks the canvas up; display scan-out is not included). Latest-wins:
// a frame for request k also answers every older request that was still waiting, with each
// request's own (longer) latency.

import { badRowStamps, sampledFnv1a } from "./integrity.ts";
import type { Frame, FramePolicy } from "./types.ts";

export interface PresentTarget {
  readonly kind: string;
  readonly adapter: string;
  draw(frame: Frame): void;
}

/** `FrameHeader.flags` bit: low-resolution scrub preview (RING.md). */
export const FLAG_LOWRES = 1;

export class Samples {
  private v: number[] = [];
  push(x: number): void {
    if (Number.isFinite(x)) this.v.push(x);
  }
  reset(): void {
    this.v = [];
  }
  get length(): number {
    return this.v.length;
  }
  static pct(v: number[], p: number): number {
    if (v.length === 0) return Number.NaN;
    const s = [...v].sort((a, b) => a - b);
    return s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))] ?? Number.NaN;
  }
  static summarize(v: number[]): LatSummary {
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
  summary(lastN?: number): LatSummary {
    return Samples.summarize(lastN === undefined ? this.v : this.v.slice(-lastN));
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
  "uploadDrawMs", // upload (writeTexture / VideoFrame + import) + draw + submit, CPU side
  "gpuDoneMs", // queue.submit -> onSubmittedWorkDone (upload + draw on the GPU, plus queueing)
  "rafIntervalMs",
] as const;
export type LatKey = (typeof LAT_KEYS)[number];

export const SCRUB_KEYS = [
  "firstSubmitMs", // input event -> queue.submit of the first frame (any resolution) for it
  "firstPixelMs", // input event -> next animation frame after that submit (scrub-to-pixel)
  "fullPixelMs", // same, for the full-resolution frame
] as const;
export type ScrubKey = (typeof SCRUB_KEYS)[number];

export interface ScrubSnapshot {
  requests: number;
  /** Requests answered by a full-resolution frame. */
  resolved: number;
  /** Requests still waiting after 3 s (dropped from the stats). */
  lost: number;
  pending: number;
  lowresPresented: number;
  latency: Record<ScrubKey, LatSummary>;
  /** Wait-in-rAF budget and the time actually spent waiting per scrub frame. */
  waitBudgetMs: number;
  waitMs: LatSummary;
}

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
  drawErrors: number;
  lastDrawError: string;
  longRafs: number;
  verify: boolean;
  format: number;
  size: string;
  lastPts: number;
  lastFlags: number;
  latency: Record<LatKey, LatSummary>;
  scrub: ScrubSnapshot;
}

/** Cheap per-frame view of what is on screen (no percentiles). */
export interface PresentedInfo {
  seq: number;
  pts: number;
  flags: number;
  width: number;
  height: number;
  format: number;
  framesPresented: number;
  framesSinceUp: number;
}

interface ScrubReq {
  pts: number;
  t0: number;
  /** performance.now() when the request was sent. */
  tSent: number;
  first: boolean;
}

const SCRUB_TIMEOUT_MS = 3000;

export class PresentLoop {
  framesPresented = 0;
  framesSinceUp = 0;
  /** Called after every presented frame (stage: resize renegotiation tracking). */
  onPresent: ((f: Frame, tSubmit: number) => void) | null = null;
  private seqGaps = 0;
  private seqViolations = 0;
  private checksumFailures = 0;
  private rowStampFailures = 0;
  private drawErrors = 0;
  private lastDrawError = "";
  private longRafs = 0;
  private startedAt = performance.now();
  private lastSeq = 0;
  private lastPts = 0;
  private lastFlags = 0;
  private lastFormat = 0;
  private lastW = 0;
  private lastH = 0;
  private lastRaf = 0;
  private running = false;
  private readonly lat = Object.fromEntries(LAT_KEYS.map((k) => [k, new Samples()])) as Record<
    LatKey,
    Samples
  >;
  private readonly scrubLat = Object.fromEntries(
    SCRUB_KEYS.map((k) => [k, new Samples()]),
  ) as Record<ScrubKey, Samples>;
  private scrubReqs: ScrubReq[] = [];
  private awaitingPixel: { t0: number; full: boolean }[] = [];
  private scrubRequests = 0;
  private scrubResolved = 0;
  private scrubLost = 0;
  private lowresPresented = 0;
  /** Scrub-to-pixel samples since the last `takeScrubSamples()` (per-second metrics log). */
  private recentScrub: number[] = [];
  /** Max ms the rAF callback may wait for a just-requested scrub frame (0 = never wait). */
  scrubWaitMs = 0;
  private readonly scrubSpin = new Samples();

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
    this.drawErrors = 0;
    this.longRafs = 0;
    this.scrubRequests = 0;
    this.scrubResolved = 0;
    this.scrubLost = 0;
    this.lowresPresented = 0;
    this.scrubReqs = [];
    this.awaitingPixel = [];
    this.recentScrub = [];
    this.scrubSpin.reset();
    this.startedAt = performance.now();
    for (const k of LAT_KEYS) this.lat[k].reset();
    for (const k of SCRUB_KEYS) this.scrubLat[k].reset();
  }

  /** Record a latency sample measured outside the loop (e.g. GPU work done). */
  sample(key: LatKey, ms: number): void {
    this.lat[key].push(ms);
  }

  /** A seek to `pts` was requested by an input event at `t0` (performance.now() timeline). */
  noteScrub(pts: number, t0: number): void {
    this.scrubRequests++;
    this.scrubReqs.push({ pts, t0, tSent: performance.now(), first: false });
  }

  /** A scrub request sent within the last frame is still waiting for its first frame. */
  private scrubJustSent(now: number): boolean {
    const r = this.scrubReqs[this.scrubReqs.length - 1];
    return r !== undefined && !r.first && now - r.tSent < 17;
  }

  takeScrubSamples(): number[] {
    const out = this.recentScrub;
    this.recentScrub = [];
    return out;
  }

  presented(): PresentedInfo {
    return {
      seq: this.lastSeq,
      pts: this.lastPts,
      flags: this.lastFlags,
      width: this.lastW,
      height: this.lastH,
      format: this.lastFormat,
      framesPresented: this.framesPresented,
      framesSinceUp: this.framesSinceUp,
    };
  }

  private resolveScrubs(f: Frame, tSubmit: number): void {
    const reqs = this.scrubReqs;
    if (reqs.length === 0) return;
    let k = -1;
    for (let i = reqs.length - 1; i >= 0; i--) {
      if ((reqs[i] as ScrubReq).pts === f.pts) {
        k = i;
        break;
      }
    }
    if (k >= 0) {
      const lowres = (f.flags & FLAG_LOWRES) !== 0;
      for (let i = 0; i <= k; i++) {
        const r = reqs[i] as ScrubReq;
        if (!r.first) {
          r.first = true;
          this.scrubLat.firstSubmitMs.push(tSubmit - r.t0);
          this.awaitingPixel.push({ t0: r.t0, full: false });
        }
        if (!lowres) this.awaitingPixel.push({ t0: r.t0, full: true });
      }
      if (!lowres) {
        reqs.splice(0, k + 1);
        this.scrubResolved += k + 1;
      }
    }
    while (reqs.length > 0 && tSubmit - (reqs[0] as ScrubReq).t0 > SCRUB_TIMEOUT_MS) {
      reqs.shift();
      this.scrubLost++;
    }
  }

  private readonly tick = (now: number): void => {
    requestAnimationFrame(this.tick); // first, so an exception below cannot stop the loop
    // The previous frame's submit is now being composited: that is the "pixel" time.
    for (const a of this.awaitingPixel) {
      const ms = now - a.t0;
      if (a.full) this.scrubLat.fullPixelMs.push(ms);
      else {
        this.scrubLat.firstPixelMs.push(ms);
        this.recentScrub.push(Math.round(ms * 100) / 100);
      }
    }
    this.awaitingPixel.length = 0;
    if (this.lastRaf) {
      const d = now - this.lastRaf;
      this.lat.rafIntervalMs.push(d);
      if (d > 25) this.longRafs++;
    }
    this.lastRaf = now;
    try {
      this.present();
    } catch (e) {
      this.drawErrors++;
      this.lastDrawError = String((e as Error)?.message ?? e).slice(0, 200);
    }
  };

  private present(): void {
    const t0 = performance.now();
    let f = this.getFrame(this.lastSeq, this.policy);
    if (!f && this.scrubWaitMs > 0 && this.scrubJustSent(t0)) {
      // Opt-in experiment: pointer events are dispatched right before rAF callbacks, so the
      // frame for a seek sent in this frame lands a few ms too late for it. Waiting (bounded)
      // for it here saves a whole frame of scrub latency at the cost of main-thread time.
      const until = t0 + this.scrubWaitMs;
      while (!f && performance.now() < until) f = this.getFrame(this.lastSeq, this.policy);
      this.scrubSpin.push(performance.now() - t0);
    }
    const t1 = performance.now();
    if (!f) return;
    if (this.lastSeq > 0 && f.seq <= this.lastSeq) this.seqViolations++;
    if (this.lastSeq > 0 && f.seq > this.lastSeq + 1) this.seqGaps += f.seq - this.lastSeq - 1;
    let t2 = t1;
    if (this.verify) {
      const bytes = new Uint8Array(f.pixels);
      if (sampledFnv1a(bytes, f.payloadLen) !== f.checksum) this.checksumFailures++;
      if (badRowStamps(bytes, f.stride, f.height, f.seq) !== 0) this.rowStampFailures++;
      t2 = performance.now();
    }
    // Accept the frame before drawing: a draw error must not make the loop re-read it forever.
    this.lastSeq = f.seq;
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
    this.lastPts = f.pts;
    this.lastFlags = f.flags ?? 0;
    this.lastFormat = f.format;
    this.lastW = f.width;
    this.lastH = f.height;
    if (this.lastFlags & FLAG_LOWRES) this.lowresPresented++;
    this.resolveScrubs(f, t3);
    this.onPresent?.(f, t3);
  }

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
      drawErrors: this.drawErrors,
      lastDrawError: this.lastDrawError,
      longRafs: this.longRafs,
      verify: this.verify,
      format: this.lastFormat,
      size: `${this.lastW}x${this.lastH}`,
      lastPts: this.lastPts,
      lastFlags: this.lastFlags,
      latency: Object.fromEntries(LAT_KEYS.map((k) => [k, this.lat[k].summary(recentN)])) as Record<
        LatKey,
        LatSummary
      >,
      scrub: {
        requests: this.scrubRequests,
        resolved: this.scrubResolved,
        lost: this.scrubLost,
        pending: this.scrubReqs.length,
        lowresPresented: this.lowresPresented,
        latency: Object.fromEntries(
          SCRUB_KEYS.map((k) => [k, this.scrubLat[k].summary()]),
        ) as Record<ScrubKey, LatSummary>,
        waitBudgetMs: this.scrubWaitMs,
        waitMs: this.scrubSpin.summary(),
      },
    };
  }
}
