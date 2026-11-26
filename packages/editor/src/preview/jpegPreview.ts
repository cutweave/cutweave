// SPDX-License-Identifier: GPL-3.0-or-later
// The reduced preview path (arch §10 fallback, WP-1M.04): when WebGPU is unavailable or fails,
// when the bridge addon or the engine's frame ring is missing, or when `CUTWEAVE_PREVIEW=jpeg`
// forces it (CI machines without a GPU), the preview polls still frames from the engine's
// `/media/frame` endpoint into a 2D canvas, at most 15 frames a second.
//
// - Playing: one request at a time, at most every 1/15 s, for the frame the clock will reach
//   when the answer is expected (`lead`, the measured fetch time); answered frames wait in a
//   small buffer and each one is drawn when the clock reaches its pts (`selectFrame`).
// - Stopped: the frame at the playhead, fetched once; a newer playhead aborts the request in
//   flight (latest wins).
// - Errors back off for a second; an aborted request is not an error.
//
// The endpoint is WP-1I.03's; until it exists `FrameFetcher` is the seam (tests use a fake).
import { FLICKS_PER_SECOND, type FrameRate, snapToFrame } from "@cutweave/timeline";
import { windowOf } from "../shell/windows.ts";
import { type PlayheadSample, playheadAt, selectFrame, staleFrames } from "./presenter.ts";

/** The fallback's ceiling (arch §10: "<= 15 fps"). */
export const FALLBACK_MAX_FPS = 15;
/** Wait after a failed request before the next one. */
export const FALLBACK_RETRY_MS = 1000;
/** Decoded frames kept ahead of the clock (the shown one included). */
const MAX_HELD = 4;
/** The endpoint's size bound (WP-1I.03: `w <= 4096`). */
export const MAX_FRAME_EDGE = 4096;

export interface FrameRequest {
  /** Timeline time in flicks, on the sequence's frame grid. */
  readonly t: number;
  /** Device pixels, even. */
  readonly width: number;
  readonly height: number;
  readonly signal: AbortSignal;
}

/** A decoded frame; `close()` releases it (an `ImageBitmap`). */
export interface DecodedFrame {
  readonly pts: number;
  readonly image: CanvasImageSource;
  readonly width: number;
  readonly height: number;
  close(): void;
}

/** Where frames come from: `/media/frame` in the app ({@link httpFrameFetcher}), a fake in tests. */
export interface FrameFetcher {
  fetchFrame(request: FrameRequest): Promise<DecodedFrame>;
}

/** Where frames go: the 2D canvas, scaled to its backing size. */
export interface FrameSurface {
  draw(frame: DecodedFrame): void;
  clear(): void;
}

/** `requestAnimationFrame` and `performance.now()`; tests pass a manual one. */
export interface PreviewTicker {
  now(): number;
  schedule(callback: () => void): number;
  cancel(handle: number): void;
}

/**
 * The frames of the window `node()` is in (a detached Preview's own, WP-2H.15: a hidden main
 * window's frames stop), read at each tick.
 */
export function windowTicker(node: () => Node | null): PreviewTicker {
  let asked: Window = windowOf(null);
  return {
    now: () => performance.now(),
    schedule: (callback) => {
      asked = windowOf(node());
      return asked.requestAnimationFrame(() => callback());
    },
    cancel: (handle) => {
      try {
        asked.cancelAnimationFrame(handle);
      } catch {
        // Its window closed, and the frame with it.
      }
    },
  };
}

export const browserTicker: PreviewTicker = windowTicker(() => null);

/** A failed frame request: the HTTP status (0 without one) and what went wrong. */
export class FrameFetchError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = "FrameFetchError";
    this.status = status;
  }
}

export interface JpegPreviewDeps {
  readonly fetcher: FrameFetcher;
  readonly surface: FrameSurface;
  readonly ticker?: PreviewTicker;
  /** The latest playhead sample (`playbackStore`). */
  readonly playhead: () => PlayheadSample;
  /** The canvas backing size in device pixels. */
  readonly size: () => { readonly width: number; readonly height: number };
  /** The sequence frame rate (requests land on its grid); `null` when unknown. */
  readonly frameRate: () => FrameRate | null;
  readonly maxFps?: number;
}

export interface JpegPreviewStats {
  readonly running: boolean;
  readonly framesFetched: number;
  readonly framesPresented: number;
  readonly errors: number;
  readonly lastError: string | null;
  /** Status of the last failed request (0: no HTTP answer). */
  readonly lastStatus: number;
  readonly lastPts: number;
  /** Measured request time (ms, smoothed). */
  readonly fetchMs: number;
}

interface InFlight {
  readonly t: number;
  readonly controller: AbortController;
  readonly startedAt: number;
}

const isAbort = (e: unknown): boolean =>
  typeof e === "object" && e !== null && (e as { name?: unknown }).name === "AbortError";

/** Even device-pixel request size within the endpoint's bound, aspect kept. */
export function requestSize(width: number, height: number): { width: number; height: number } {
  const scale = Math.min(1, MAX_FRAME_EDGE / Math.max(width, height, 1));
  const even = (v: number) => Math.max(2, Math.round((v * scale) / 2) * 2);
  return { width: even(width), height: even(height) };
}

export class JpegPreview {
  readonly #deps: JpegPreviewDeps;
  readonly #ticker: PreviewTicker;
  readonly #interval: number;
  #handle: number | null = null;
  #held: DecodedFrame[] = [];
  #shown: DecodedFrame | null = null;
  #inflight: InFlight | null = null;
  #lastTarget: number | null = null;
  #lastStart = Number.NEGATIVE_INFINITY;
  #retryAt = 0;
  #invalid = false;
  #fetchMs: number;
  #framesFetched = 0;
  #framesPresented = 0;
  #errors = 0;
  #lastError: string | null = null;
  #lastStatus = 0;
  #lastPts = 0;

  constructor(deps: JpegPreviewDeps) {
    this.#deps = deps;
    this.#ticker = deps.ticker ?? browserTicker;
    this.#interval = 1000 / (deps.maxFps ?? FALLBACK_MAX_FPS);
    this.#fetchMs = this.#interval;
  }

  get running(): boolean {
    return this.#handle !== null;
  }

  start(): void {
    if (this.#handle !== null) return;
    this.#handle = this.#ticker.schedule(this.#tick);
  }

  /** Stops polling, aborts the request in flight and releases every held frame. */
  stop(): void {
    if (this.#handle !== null) this.#ticker.cancel(this.#handle);
    this.#handle = null;
    this.#inflight?.controller.abort();
    this.#inflight = null;
    for (const f of this.#held) f.close();
    this.#held = [];
    this.#shown = null;
    this.#lastTarget = null;
  }

  /** The picture at the playhead changed (an edit, a resize): fetch the current frame again. */
  invalidate(): void {
    this.#invalid = true;
  }

  /** Draws the shown frame again (a resize cleared the canvas); false when there is none. */
  redraw(): boolean {
    if (this.#shown === null) return false;
    this.#deps.surface.draw(this.#shown);
    return true;
  }

  stats(): JpegPreviewStats {
    return {
      running: this.running,
      framesFetched: this.#framesFetched,
      framesPresented: this.#framesPresented,
      errors: this.#errors,
      lastError: this.#lastError,
      lastStatus: this.#lastStatus,
      lastPts: this.#lastPts,
      fetchMs: Math.round(this.#fetchMs * 10) / 10,
    };
  }

  readonly #tick = (): void => {
    if (this.#handle === null) return;
    const now = this.#ticker.now();
    const sample = this.#deps.playhead();
    const rate = sample.playing ? sample.rate : 0;
    const clock = playheadAt(sample, now);
    if (now >= this.#retryAt) {
      if (rate === 0) this.#requestStill(sample.t, now);
      else this.#requestAhead(clock, rate, now);
    }
    this.#present(clock, rate);
    this.#handle = this.#ticker.schedule(this.#tick);
  };

  #snap(t: number): number {
    const fps = this.#deps.frameRate();
    return Math.max(0, fps === null ? Math.round(t) : snapToFrame(t, fps, "floor"));
  }

  #requestStill(playhead: number, now: number): void {
    const target = this.#snap(playhead);
    if (target === this.#lastTarget && !this.#invalid) return;
    // Latest wins, but at most one new request per polling interval.
    if (this.#inflight !== null && now - this.#lastStart < this.#interval - 1) return;
    this.#fetch(target, now);
  }

  #requestAhead(clock: number, rate: number, now: number): void {
    if (this.#inflight !== null || now - this.#lastStart < this.#interval - 1) return;
    if (this.#held.length >= MAX_HELD) return;
    const lead = Math.max(this.#fetchMs, this.#interval);
    const target = this.#snap(clock + (lead * rate * FLICKS_PER_SECOND) / 1000);
    if (target === this.#lastTarget && !this.#invalid) return;
    this.#fetch(target, now);
  }

  #fetch(t: number, now: number): void {
    const { width, height } = this.#deps.size();
    if (width <= 0 || height <= 0) return;
    this.#inflight?.controller.abort();
    const flight: InFlight = { t, controller: new AbortController(), startedAt: now };
    this.#inflight = flight;
    this.#lastTarget = t;
    this.#lastStart = now;
    this.#invalid = false;
    const size = requestSize(width, height);
    this.#deps.fetcher.fetchFrame({ t, ...size, signal: flight.controller.signal }).then(
      (frame) => {
        if (this.#inflight !== flight || this.#handle === null) {
          frame.close();
          return;
        }
        this.#inflight = null;
        this.#framesFetched += 1;
        const took = this.#ticker.now() - flight.startedAt;
        this.#fetchMs = this.#fetchMs * 0.7 + took * 0.3;
        this.#held.push(frame);
        while (this.#held.length > MAX_HELD) this.#held.shift()?.close();
      },
      (error: unknown) => {
        if (this.#inflight === flight) this.#inflight = null;
        if (isAbort(error) || this.#handle === null) return;
        this.#errors += 1;
        this.#lastError = error instanceof Error ? error.message : String(error);
        this.#lastStatus = error instanceof FrameFetchError ? error.status : 0;
        this.#retryAt = this.#ticker.now() + FALLBACK_RETRY_MS;
        // Ask again after the back-off even if the playhead has not moved.
        this.#lastTarget = null;
      },
    );
  }

  #present(clock: number, rate: number): void {
    let due: DecodedFrame | null;
    if (rate === 0) {
      // Stopped: only the frame of the current target counts (a late answer for an older seek
      // never shows).
      due = this.#held.findLast((f) => f.pts === this.#lastTarget) ?? null;
    } else {
      due = selectFrame(this.#held, clock, { rate });
    }
    if (due !== null && due !== this.#shown) {
      this.#deps.surface.draw(due);
      this.#shown = due;
      this.#framesPresented += 1;
      this.#lastPts = due.pts;
    }
    const drop =
      rate === 0
        ? this.#held.filter((f) => f !== this.#shown && f.pts !== this.#lastTarget)
        : staleFrames(this.#held, this.#shown, rate);
    if (drop.length > 0) {
      for (const f of drop) f.close();
      this.#held = this.#held.filter((f) => !drop.includes(f));
    }
  }
}

export interface HttpFrameFetcherOptions {
  /** The engine's HTTP origin and UI token (`window.cutweave.engine.getConnection()`). */
  readonly connection: () => Promise<{ readonly httpUrl: string; readonly token: string } | null>;
  /** The sequence to render (`seq` param); `null` lets the engine pick its active one. */
  readonly sequence: () => string | null;
  readonly fetch?: typeof fetch;
  readonly decode?: (blob: Blob) => Promise<ImageBitmap>;
}

/** `GET {httpUrl}/media/frame?seq=&t=&w=&h=&fmt=jpeg` (WP-1I.03's route). */
export function frameUrl(
  httpUrl: string,
  sequence: string | null,
  request: { readonly t: number; readonly width: number; readonly height: number },
): string {
  const params = new URLSearchParams();
  if (sequence !== null) params.set("seq", sequence);
  params.set("t", String(Math.round(request.t)));
  params.set("w", String(request.width));
  params.set("h", String(request.height));
  params.set("fmt", "jpeg");
  return `${httpUrl.replace(/\/+$/, "")}/media/frame?${params}`;
}

/**
 * Frames from the engine over HTTP. The UI token goes in the `Authorization` header, never in
 * the URL (shared/api.ts); the answer must be an image.
 */
export function httpFrameFetcher(options: HttpFrameFetcherOptions): FrameFetcher {
  const doFetch = options.fetch ?? ((input, init) => globalThis.fetch(input, init));
  const decode = options.decode ?? ((blob: Blob) => createImageBitmap(blob));
  return {
    async fetchFrame(request) {
      const conn = await options.connection();
      if (conn === null) throw new FrameFetchError(0, "the engine is not connected");
      const response = await doFetch(frameUrl(conn.httpUrl, options.sequence(), request), {
        headers: { Authorization: `Bearer ${conn.token}` },
        signal: request.signal,
        cache: "no-store",
      });
      const type = response.headers.get("content-type") ?? "";
      if (!response.ok || !type.startsWith("image/")) {
        throw new FrameFetchError(response.status, `HTTP ${response.status} from /media/frame`);
      }
      const bitmap = await decode(await response.blob());
      return {
        pts: request.t,
        image: bitmap,
        width: bitmap.width,
        height: bitmap.height,
        close: () => bitmap.close(),
      };
    },
  };
}

/** Draws frames into a 2D canvas, scaled to its backing size. */
export function canvasSurface(canvas: HTMLCanvasElement): FrameSurface | null {
  const ctx = canvas.getContext("2d", { alpha: false });
  if (ctx === null) return null;
  return {
    draw(frame) {
      ctx.drawImage(frame.image, 0, 0, canvas.width, canvas.height);
    },
    clear() {
      ctx.clearRect(0, 0, canvas.width, canvas.height);
    },
  };
}
