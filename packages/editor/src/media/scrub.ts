// SPDX-License-Identifier: GPL-3.0-or-later
// Scrubbing media before it is placed (WP-1M.14): the pure parts. A pointer position becomes a
// frame of the asset's own grid (time stays flicks, via the timeline's helpers), and
// `LatestFrames` makes sure the screen only ever shows the newest answer: at most one load per
// animation frame, a load still running is aborted when a newer one starts, a late answer is
// dropped, and nothing is asked once the pointer has left.
import { type FrameRate, frameDurationFlicks, frameToFlicks } from "@cutweave/timeline";

/** The rate assumed for assets without one (sound, stills). */
export const DEFAULT_RATE: FrameRate = { num: 30, den: 1 };

export interface FrameGrid {
  readonly rate: FrameRate;
  /** How many frames the asset has (at least 1). */
  readonly frames: number;
}

/** The frame grid of an asset of `duration` flicks at `fps` (30 when it has none). */
export function frameGrid(duration: number | null, fps: FrameRate | null): FrameGrid {
  const rate = fps === null || fps.num <= 0 || fps.den <= 0 ? DEFAULT_RATE : fps;
  if (duration === null || duration <= 0) return { rate, frames: 1 };
  return { rate, frames: Math.max(1, Math.ceil(duration / frameDurationFlicks(rate))) };
}

const clamp01 = (x: number): number => (Number.isFinite(x) ? Math.min(1, Math.max(0, x)) : 0);

/** The frame (0-based) at `fraction` of the asset's length: the first for 0, the last for 1. */
export function frameAtFraction(fraction: number, grid: FrameGrid): number {
  return Math.min(grid.frames - 1, Math.floor(clamp01(fraction) * grid.frames));
}

/** Where on the bar a frame sits (its centre), 0..1. */
export function fractionOfFrame(frame: number, grid: FrameGrid): number {
  return Math.min(1, Math.max(0, (frame + 0.5) / grid.frames));
}

/** Media time of a frame, flicks. */
export function timeOfFrame(frame: number, grid: FrameGrid): number {
  return frameToFlicks(Math.min(grid.frames - 1, Math.max(0, frame)), grid.rate);
}

/** The pointer's place along a box, 0..1 (0 for a box with no width). */
export function pointerFraction(clientX: number, left: number, width: number): number {
  return width <= 0 ? 0 : clamp01((clientX - left) / width);
}

export interface LatestFramesOptions<T> {
  /** Loads the picture for media time `t` (flicks); `null` for "nothing to show". */
  readonly load: (t: number, signal: AbortSignal) => Promise<T | null>;
  /** Shows a loaded picture; only ever called for the newest request. */
  readonly paint: (t: number, picture: T) => void;
  /** Schedules `run` for the next animation frame (default: `requestAnimationFrame`). */
  readonly raf?: (run: () => void) => number;
  readonly cancelRaf?: (handle: number) => void;
}

/** Latest-wins loading of the frame under a moving pointer. */
export class LatestFrames<T> {
  readonly #options: LatestFramesOptions<T>;
  #wanted: number | null = null;
  #started: number | null = null;
  #handle: number | null = null;
  #current: AbortController | null = null;
  #sequence = 0;

  constructor(options: LatestFramesOptions<T>) {
    this.#options = options;
  }

  /** Asks for the frame at `t`; the load starts on the next animation frame. */
  request(t: number): void {
    this.#wanted = t;
    if (this.#handle !== null) return;
    const raf = this.#options.raf ?? ((run) => requestAnimationFrame(() => run()));
    this.#handle = raf(() => {
      this.#handle = null;
      this.#flush();
    });
  }

  /** Stops: a load in flight is aborted, a scheduled one never starts, an answer is dropped. */
  cancel(): void {
    if (this.#handle !== null) {
      const cancelRaf = this.#options.cancelRaf ?? ((h) => cancelAnimationFrame(h));
      cancelRaf(this.#handle);
      this.#handle = null;
    }
    this.#wanted = null;
    this.#started = null;
    this.#sequence += 1;
    this.#current?.abort();
    this.#current = null;
  }

  #flush(): void {
    const t = this.#wanted;
    this.#wanted = null;
    if (t === null || t === this.#started) return;
    this.#started = t;
    this.#current?.abort();
    const controller = new AbortController();
    this.#current = controller;
    const mine = ++this.#sequence;
    this.#options.load(t, controller.signal).then(
      (picture) => {
        if (mine !== this.#sequence || controller.signal.aborted || picture === null) return;
        this.#options.paint(t, picture);
      },
      () => {
        // Aborted, or the engine could not make it: the previous picture stays.
        if (mine === this.#sequence) this.#started = null;
      },
    );
  }
}

/**
 * Draws `source` (or the `sw`×`sh` part of it at `sx`,`sy`) into `canvas`, covering it like
 * `object-fit: cover`. The canvas's bitmap is sized to its box at the device's pixel ratio.
 */
export function paintCover(
  canvas: HTMLCanvasElement,
  source: CanvasImageSource,
  size: { readonly width: number; readonly height: number },
  part?: { readonly sx: number; readonly sy: number; readonly sw: number; readonly sh: number },
): void {
  const ratio = globalThis.devicePixelRatio || 1;
  const width = Math.max(1, Math.round(canvas.clientWidth * ratio));
  const height = Math.max(1, Math.round(canvas.clientHeight * ratio));
  if (canvas.width !== width) canvas.width = width;
  if (canvas.height !== height) canvas.height = height;
  const ctx = canvas.getContext("2d");
  if (ctx === null) return;
  const sx = part?.sx ?? 0;
  const sy = part?.sy ?? 0;
  const sw = part?.sw ?? size.width;
  const sh = part?.sh ?? size.height;
  const scale = Math.max(width / sw, height / sh);
  const cw = width / scale;
  const ch = height / scale;
  ctx.drawImage(source, sx + (sw - cw) / 2, sy + (sh - ch) / 2, cw, ch, 0, 0, width, height);
}
