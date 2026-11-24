// SPDX-License-Identifier: GPL-3.0-or-later
// The preview present loop, in the preload's isolated world (WP-1H.03). Spike S1 findings 2 and 4:
//   - pixels must not cross `contextBridge` per frame (a structured clone, 4-9 ms for 1080p RGBA8
//     on the renderer main thread), so ring read and upload both run here and only small stats
//     cross to the page;
//   - frame choice follows the audio clock, "the newest frame with pts <= clock - latency",
//     which presented 60.00 fps with no gaps where latest-wins in rAF dropped 6-320 frames/min.
// WP-1M.04 plugs its WebGPU presenter in through `setFramePresenter` (webgpu-presenter.ts); with
// none installed, frames are counted and verified but not drawn. The loop also counts display
// ticks (the presenter instrumentation of the 60 s playback check: ticks per second, late ticks,
// frames the engine published that were never presented).

import type { PresentOptions, PresentStats, PreviewFrame } from "../shared/api.ts";

/** Default clock margin (ms): 20 ms gave 0 skipped frames at 60 fps in Spike S1. */
export const DEFAULT_LATENCY_MS = 20;

/** A tick later than this after the previous one is counted as late (1.5 frames at 60 Hz). */
export const LONG_TICK_MS = 25;

/** What a presenter reports about itself (small plain values). */
export interface FramePresenterStats {
  readonly kind: "webgpu";
  readonly errors: number;
  readonly lost: boolean;
  readonly lastError: string | null;
}

/** Draws a frame (WP-1M.04: upload to a WebGPU texture and render). Runs in the preload. */
export interface FramePresenter {
  /** `frame.bytes` is valid until the second next frame; upload synchronously. */
  present(frame: PreviewFrame): void;
  /** Instrumentation, when the presenter has any. */
  stats?(): FramePresenterStats | undefined;
}

/** What the loop reads from the addon. */
export interface FrameSource {
  frameAtClock(latencyMs: number): PreviewFrame | null;
}

export interface LoopClock {
  /** Schedule one tick (`requestAnimationFrame`); returns a handle for `cancel`. */
  schedule(tick: () => void): number;
  cancel(handle: number): void;
  /** Milliseconds, monotonic (`performance.now`). */
  now(): number;
}

let installed: FramePresenter | null = null;

/** Install the presenter every future `startPresenting` uses (WP-1M.04). `null` = none. */
export function setFramePresenter(presenter: FramePresenter | null): void {
  installed = presenter;
}

export function currentFramePresenter(): FramePresenter | null {
  return installed;
}

/**
 * A presenter that forwards to whichever one is installed when a frame arrives, so a canvas
 * attached (or detached) while the loop runs takes effect at once.
 */
export const installedPresenter: FramePresenter = {
  present: (frame) => installed?.present(frame),
  stats: () => installed?.stats?.(),
};

/** Timestamps of the last second, in a fixed ring (no allocation per tick). */
class Window {
  readonly #times = new Float64Array(512);
  #head = 0;
  #size = 0;

  push(t: number): void {
    this.#times[this.#head] = t;
    this.#head = (this.#head + 1) % this.#times.length;
    if (this.#size < this.#times.length) this.#size += 1;
  }

  /** Entries within `ms` before `now`. */
  countSince(now: number, ms: number): number {
    let n = 0;
    for (let i = 0; i < this.#size; i++) {
      const t = this.#times[(this.#head - 1 - i + this.#times.length) % this.#times.length] ?? 0;
      if (now - t > ms) break;
      n += 1;
    }
    return n;
  }
}

/** The present loop: one `tick` per display frame. Testable with a fake clock and source. */
export class PresentLoop {
  private handle: number | null = null;
  private framesPresented = 0;
  private seqGaps = 0;
  private errors = 0;
  private lastError: string | null = null;
  private lastSeq = 0;
  private lastPts = 0;
  private lastWidth = 0;
  private lastHeight = 0;
  private lastFormat: PreviewFrame["format"] | null = null;
  private ticks = 0;
  private longTicks = 0;
  private lastTick: number | null = null;
  private readonly presents = new Window();
  private readonly tickTimes = new Window();

  constructor(
    private readonly source: FrameSource,
    private readonly presenter: FramePresenter | null,
    private readonly clock: LoopClock,
    private readonly options: PresentOptions = {},
  ) {}

  get running(): boolean {
    return this.handle !== null;
  }

  start(): void {
    if (this.handle !== null) return;
    this.lastTick = null;
    this.handle = this.clock.schedule(this.tick);
  }

  stop(): void {
    if (this.handle !== null) this.clock.cancel(this.handle);
    this.handle = null;
  }

  private readonly tick = (): void => {
    if (this.handle === null) return;
    const now = this.clock.now();
    this.ticks += 1;
    if (this.lastTick !== null && now - this.lastTick > LONG_TICK_MS) this.longTicks += 1;
    this.lastTick = now;
    this.tickTimes.push(now);
    try {
      const latency = this.options.latencyMs ?? DEFAULT_LATENCY_MS;
      const frame = this.source.frameAtClock(latency);
      if (frame !== null) this.deliver(frame, now);
    } catch (err) {
      // A closed or corrupt ring ends the loop; the page sees it in `getPresentStats()` and
      // `getStatus()`, and restarts presenting after re-opening the ring.
      this.errors += 1;
      this.lastError = err instanceof Error ? err.message : String(err);
      this.handle = null;
      return;
    }
    this.handle = this.clock.schedule(this.tick);
  };

  private deliver(frame: PreviewFrame, now: number): void {
    if (this.lastSeq > 0 && frame.seq > this.lastSeq + 1)
      this.seqGaps += frame.seq - this.lastSeq - 1;
    this.lastSeq = frame.seq;
    this.presenter?.present(frame);
    this.framesPresented += 1;
    this.presents.push(now);
    // Keep only small facts: the frame's pixels belong to the (pooled) addon buffer.
    this.lastPts = frame.pts;
    this.lastWidth = frame.width;
    this.lastHeight = frame.height;
    this.lastFormat = frame.format;
  }

  stats(): PresentStats {
    const now = this.clock.now();
    const p = this.presenter?.stats?.();
    return {
      running: this.running,
      framesPresented: this.framesPresented,
      seqGaps: this.seqGaps,
      lastSeq: this.lastSeq,
      lastPts: this.lastPts,
      lastWidth: this.lastWidth,
      lastHeight: this.lastHeight,
      lastFormat: this.lastFormat,
      presentFps: this.presents.countSince(now, 1000),
      errors: this.errors,
      lastError: this.lastError,
      ticks: this.ticks,
      tickFps: this.tickTimes.countSince(now, 1000),
      longTicks: this.longTicks,
      presenter: p?.kind ?? "none",
      presenterErrors: p?.errors ?? 0,
      presenterLost: p?.lost ?? false,
      presenterError: p?.lastError ?? null,
    };
  }
}

/** The loop's clock in the preload: `requestAnimationFrame` and `performance.now`. */
export function browserLoopClock(): LoopClock {
  return {
    schedule: (tick) => requestAnimationFrame(() => tick()),
    cancel: (handle) => cancelAnimationFrame(handle),
    now: () => performance.now(),
  };
}
