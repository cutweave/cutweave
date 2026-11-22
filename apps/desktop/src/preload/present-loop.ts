// SPDX-License-Identifier: GPL-3.0-or-later
// The preview present loop, in the preload's isolated world (WP-1H.03). Spike S1 findings 2 and 4:
//   - pixels must not cross `contextBridge` per frame (a structured clone, 4-9 ms for 1080p RGBA8
//     on the renderer main thread), so ring read and upload both run here and only small stats
//     cross to the page;
//   - frame choice follows the audio clock, "the newest frame with pts <= clock - latency",
//     which presented 60.00 fps with no gaps where latest-wins in rAF dropped 6-320 frames/min.
// WP-1M.04 plugs its WebGPU presenter in through `setFramePresenter`; until then frames are
// counted and verified but not drawn.

import type { PresentOptions, PresentStats, PreviewFrame } from "../shared/api.ts";

/** Default clock margin (ms): 20 ms gave 0 skipped frames at 60 fps in Spike S1. */
export const DEFAULT_LATENCY_MS = 20;

/** Draws a frame (WP-1M.04: upload to a WebGPU texture and render). Runs in the preload. */
export interface FramePresenter {
  /** `frame.bytes` is valid until the second next frame; upload synchronously. */
  present(frame: PreviewFrame): void;
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

/** The present loop: one `tick` per display frame. Testable with a fake clock and source. */
export class PresentLoop {
  private handle: number | null = null;
  private framesPresented = 0;
  private seqGaps = 0;
  private errors = 0;
  private lastError: string | null = null;
  private last: PreviewFrame | null = null;
  private lastSeq = 0;
  /** Present times (ms) within the last second. */
  private recent: number[] = [];

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
    this.handle = this.clock.schedule(this.tick);
  }

  stop(): void {
    if (this.handle !== null) this.clock.cancel(this.handle);
    this.handle = null;
  }

  private readonly tick = (): void => {
    if (this.handle === null) return;
    try {
      const latency = this.options.latencyMs ?? DEFAULT_LATENCY_MS;
      const frame = this.source.frameAtClock(latency);
      if (frame !== null) this.deliver(frame);
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

  private deliver(frame: PreviewFrame): void {
    if (this.lastSeq > 0 && frame.seq > this.lastSeq + 1)
      this.seqGaps += frame.seq - this.lastSeq - 1;
    this.lastSeq = frame.seq;
    this.presenter?.present(frame);
    this.framesPresented += 1;
    const now = this.clock.now();
    this.recent.push(now);
    while (this.recent.length > 0 && now - (this.recent[0] ?? now) > 1000) this.recent.shift();
    // Keep only small facts: the frame's pixels belong to the (pooled) addon buffer.
    this.last = { ...frame, bytes: new ArrayBuffer(0) };
  }

  stats(): PresentStats {
    const now = this.clock.now();
    const fps = this.recent.filter((t) => now - t <= 1000).length;
    return {
      running: this.running,
      framesPresented: this.framesPresented,
      seqGaps: this.seqGaps,
      lastSeq: this.lastSeq,
      lastPts: this.last?.pts ?? 0,
      lastWidth: this.last?.width ?? 0,
      lastHeight: this.last?.height ?? 0,
      lastFormat: this.last?.format ?? null,
      presentFps: fps,
      errors: this.errors,
      lastError: this.lastError,
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
