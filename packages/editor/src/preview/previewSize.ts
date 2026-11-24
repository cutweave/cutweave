// SPDX-License-Identifier: GPL-3.0-or-later
// Live resize (WP-1M.04, arch §10 step 1): the engine renders preview frames at the canvas's
// device-pixel size. The canvas backing store follows its box at once (PreviewCanvas, in the
// ResizeObserver callback, so a resized canvas is redrawn before it is painted); the engine hears
// about it here: `view.set_preview_size{width, height}` 50 ms after the last change, rounded to
// even (NV12 frames need even edges), one request at a time with the newest size winning.

/** Quiet time before a size goes to the engine. */
export const PREVIEW_SIZE_DEBOUNCE_MS = 50;

export interface SizerTimers {
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

const browserTimers: SizerTimers = {
  setTimeout: (callback, ms) => globalThis.setTimeout(callback, ms),
  clearTimeout: (handle) => globalThis.clearTimeout(handle as number),
};

export interface PixelSize {
  readonly width: number;
  readonly height: number;
}

/** Each edge rounded to the nearest even number, at least 2. */
export function evenSize(width: number, height: number): PixelSize {
  const even = (v: number) => Math.max(2, Math.round(v / 2) * 2);
  return { width: even(Math.floor(width)), height: even(Math.floor(height)) };
}

/** The slice of a `ResizeObserverEntry` the size comes from. */
export interface ObservedBox {
  readonly contentRect: { readonly width: number; readonly height: number };
  readonly devicePixelContentBoxSize?: readonly {
    readonly inlineSize: number;
    readonly blockSize: number;
  }[];
}

/**
 * The box in device pixels: the exact `device-pixel-content-box` where the browser reports it
 * (Chromium does), else the CSS size times the device pixel ratio.
 */
export function devicePixelSize(entry: ObservedBox, dpr: number): PixelSize {
  const exact = entry.devicePixelContentBoxSize?.[0];
  if (exact !== undefined) return { width: exact.inlineSize, height: exact.blockSize };
  return {
    width: Math.round(entry.contentRect.width * dpr),
    height: Math.round(entry.contentRect.height * dpr),
  };
}

export interface PreviewSizerOptions {
  /** Sends `view.set_preview_size`. */
  readonly send: (width: number, height: number) => Promise<unknown>;
  readonly timers?: SizerTimers;
  readonly debounceMs?: number;
  readonly onError?: (error: unknown) => void;
}

export class PreviewSizer {
  readonly #send: PreviewSizerOptions["send"];
  readonly #timers: SizerTimers;
  readonly #debounceMs: number;
  readonly #onError: (error: unknown) => void;
  /** The newest observed size (even). */
  #wanted: PixelSize | null = null;
  /** What the engine was last told successfully (or is being told). */
  #sent: PixelSize | null = null;
  #timer: unknown = null;
  #inflight = false;
  #again = false;

  constructor(options: PreviewSizerOptions) {
    this.#send = options.send;
    this.#timers = options.timers ?? browserTimers;
    this.#debounceMs = options.debounceMs ?? PREVIEW_SIZE_DEBOUNCE_MS;
    this.#onError = options.onError ?? ((e) => console.warn("[preview] set_preview_size", e));
  }

  /** The size last sent to the engine. */
  get requested(): PixelSize | null {
    return this.#sent;
  }

  /** The canvas now measures `width × height` device pixels. */
  observe(width: number, height: number): void {
    if (!(width > 0 && height > 0)) return;
    this.#wanted = evenSize(width, height);
    if (this.#timer !== null) this.#timers.clearTimeout(this.#timer);
    this.#timer = this.#timers.setTimeout(() => {
      this.#timer = null;
      this.#flush();
    }, this.#debounceMs);
  }

  /** Sends the current size again now (after an engine restart or reconnect). */
  resend(): void {
    this.#sent = null;
    this.#flush();
  }

  dispose(): void {
    if (this.#timer !== null) this.#timers.clearTimeout(this.#timer);
    this.#timer = null;
    this.#wanted = null;
  }

  #flush(): void {
    const wanted = this.#wanted;
    if (wanted === null) return;
    if (this.#inflight) {
      this.#again = true;
      return;
    }
    if (this.#sent?.width === wanted.width && this.#sent.height === wanted.height) return;
    this.#inflight = true;
    this.#sent = wanted;
    this.#send(wanted.width, wanted.height)
      .catch((error: unknown) => {
        // Not applied: the next change (or `resend`) sends it again.
        if (this.#sent === wanted) this.#sent = null;
        this.#onError(error);
      })
      .finally(() => {
        this.#inflight = false;
        if (this.#again) {
          this.#again = false;
          this.#flush();
        }
      });
  }
}
