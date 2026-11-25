// SPDX-License-Identifier: GPL-3.0-or-later
// The GPU preview path, page side (WP-1M.04; ADR-001 transport (a), arch §10 steps 2-3).
//
// Pixels never enter the page: the desktop preload maps the engine's shared-memory frame ring
// (cutweave-bridge), runs the present loop in its isolated world and uploads each frame to a
// WebGPU texture drawn into the page's canvas (NV12 or RGBA8 through `queue.writeTexture`, the
// upload path Spike S1 chose). The page hands over its canvas (`attachCanvas`), tells the
// preload which ring to open (the engine announces `{name, epoch}` as `shm` in its `rpc.hello`),
// and watches the ring: when it ends (the engine restarted, or moved it twice too fast to
// follow) the page asks the engine for its current ring and opens that one.
//
// Anything missing (no addon, no WebGPU adapter, no ring) is reported as a failure with a
// reason, and PreviewCanvas falls back to the JPEG path.

/** The ring the engine announced (`RingAnnouncement` in cutweave-playback, RING.md). */
export interface RingAnnouncement {
  readonly name: string;
  /** `0x` + hex or decimal digits; the addon checks it against the ring header. */
  readonly epoch: string;
}

const RING_NAME = /^[A-Za-z0-9._-]{1,64}$/;
const EPOCH = /^(0x[0-9a-fA-F]{1,16}|[0-9]{1,20})$/;

/**
 * Reads the `shm` field of `rpc.hello`: `{name, epoch, ...}` as an object or as its JSON
 * text. `null` when there is none or it is malformed.
 */
export function parseRingAnnouncement(shm: unknown): RingAnnouncement | null {
  let value = shm;
  if (typeof value === "string") {
    try {
      value = JSON.parse(value);
    } catch {
      return null;
    }
  }
  if (typeof value !== "object" || value === null) return null;
  const v = value as Record<string, unknown>;
  const epoch =
    typeof v.epoch === "string"
      ? v.epoch
      : typeof v.epoch === "number" && Number.isSafeInteger(v.epoch) && v.epoch >= 0
        ? String(v.epoch)
        : null;
  if (typeof v.name !== "string" || !RING_NAME.test(v.name)) return null;
  if (epoch === null || !EPOCH.test(epoch)) return null;
  return { name: v.name, epoch };
}

export type AttachResult =
  | { readonly ok: true; readonly adapter: string; readonly upload: string }
  | { readonly ok: false; readonly reason: string; readonly message: string };

/**
 * The part of `window.cutweave.preview` (apps/desktop `shared/api.ts`) this path uses. Throwing
 * methods throw `ERR_*`-prefixed errors.
 */
export interface PreviewBridge {
  isAvailable(): boolean;
  attachCanvas(canvas: HTMLCanvasElement): Promise<AttachResult>;
  detachCanvas(): void;
  redraw(): boolean;
  open(ringName: string, epoch: string): unknown;
  startPresenting(options?: { readonly latencyMs?: number }): void;
  stopPresenting(): void;
  getStatus(): { readonly state: "live" | "starting" | "ended"; readonly reason?: string };
  getPresentStats(): { readonly running: boolean; readonly errors: number } | null;
  close(): void;
}

export type FailureReason =
  /** The bridge addon did not load (or this is not the desktop app's main window). */
  | "no-addon"
  /** No WebGPU adapter, device or canvas context. */
  | "webgpu"
  /** The engine publishes no frame ring (an engine without the playback subsystem). */
  | "no-ring"
  /** The ring could not be opened or kept failing. */
  | "ring";

export type RingPreviewState =
  | { readonly kind: "idle" }
  | { readonly kind: "presenting"; readonly adapter: string }
  | { readonly kind: "recovering" }
  | { readonly kind: "failed"; readonly reason: FailureReason; readonly message: string };

export interface WatchTimers {
  setInterval(callback: () => void, ms: number): unknown;
  clearInterval(handle: unknown): void;
}

const browserTimers: WatchTimers = {
  setInterval: (callback, ms) => globalThis.setInterval(callback, ms),
  clearInterval: (handle) => globalThis.clearInterval(handle as number),
};

/** Spike S1's clock margin: present the newest frame with `pts <= clock - 20 ms`. */
export const PRESENT_LATENCY_MS = 20;
/** How often the ring's liveness is checked. */
export const WATCH_MS = 250;
/** Recoveries in a row that may fail before the path gives up. */
const MAX_RECOVERIES = 3;

export interface RingPreviewDeps {
  readonly bridge: PreviewBridge;
  /** Asks the engine for its current ring (`rpc.hello` again); `null` when it has none. */
  readonly announce: () => Promise<RingAnnouncement | null>;
  readonly timers?: WatchTimers;
  readonly latencyMs?: number;
  readonly onState?: (state: RingPreviewState) => void;
}

const message = (e: unknown): string => (e instanceof Error ? e.message : String(e));

export class RingPreview {
  readonly #deps: RingPreviewDeps;
  readonly #timers: WatchTimers;
  #state: RingPreviewState = { kind: "idle" };
  #adapter = "";
  #ring: RingAnnouncement | null = null;
  #watch: unknown = null;
  #recovering = false;
  #failures = 0;
  #lastError = "";
  #attached = false;
  #attaching = false;
  /** Presenting is held: the canvas keeps the frame it shows (the Inspector draws the source). */
  #paused = false;
  /** Bumped by every attach and detach: a slower, older attach must not take effect. */
  #generation = 0;

  constructor(deps: RingPreviewDeps) {
    this.#deps = deps;
    this.#timers = deps.timers ?? browserTimers;
  }

  get state(): RingPreviewState {
    return this.#state;
  }

  /** Hands `canvas` to the preload's WebGPU presenter and presents `ring`. */
  async attach(
    canvas: HTMLCanvasElement,
    ring: RingAnnouncement | null,
  ): Promise<RingPreviewState> {
    const { bridge } = this.#deps;
    if (!bridge.isAvailable()) {
      return this.#fail("no-addon", "the preview bridge is not available");
    }
    const generation = ++this.#generation;
    this.#attaching = true;
    let attached: AttachResult;
    try {
      attached = await bridge.attachCanvas(canvas);
    } catch (e) {
      if (generation !== this.#generation) return this.#state;
      return this.#fail("webgpu", message(e));
    } finally {
      if (generation === this.#generation) this.#attaching = false;
    }
    // Detached (or attached again) while the GPU device was being created: that call won.
    if (generation !== this.#generation) return this.#state;
    if (!attached.ok) return this.#fail("webgpu", attached.message);
    this.#attached = true;
    this.#adapter = attached.adapter;
    if (ring === null) return this.#fail("no-ring", "the engine publishes no preview frames");
    // The announced ring may have moved already (a resize right after the handshake): then the
    // watch asks the engine for its current one.
    if (this.#open(ring) !== null) this.#failed(this.#lastError);
    this.#startWatch();
    return this.#state;
  }

  /** A newer announcement (reconnect, engine restart): switch when it names another ring. */
  setRing(ring: RingAnnouncement | null): void {
    if (!this.#attached || ring === null || this.#isCurrent(ring)) return;
    if (this.#open(ring) !== null) this.#failed(this.#lastError);
    this.#startWatch();
  }

  /** The ring ended (writer gone, stale, moved twice) or the present loop stopped on an error. */
  #ended(): boolean {
    const { bridge } = this.#deps;
    try {
      const status = bridge.getStatus();
      const stats = bridge.getPresentStats();
      return status.state === "ended" || (stats !== null && !stats.running && stats.errors > 0);
    } catch {
      return true;
    }
  }

  #isCurrent(ring: RingAnnouncement): boolean {
    return ring.name === this.#ring?.name && ring.epoch === this.#ring.epoch;
  }

  /** Holds the picture: stops presenting, the canvas keeps its last frame (WP-1M.15). */
  pause(): void {
    if (this.#paused) return;
    this.#paused = true;
    if (!this.#attached || this.#ring === null) return;
    try {
      this.#deps.bridge.stopPresenting();
    } catch {
      // Already stopped or gone.
    }
  }

  /** Presents the ring again after {@link pause}. */
  resume(): void {
    if (!this.#paused) return;
    this.#paused = false;
    if (!this.#attached || this.#ring === null) return;
    try {
      this.#deps.bridge.startPresenting({ latencyMs: this.#deps.latencyMs ?? PRESENT_LATENCY_MS });
    } catch (e) {
      this.#failed(message(e));
    }
  }

  /** Repaints the last frame (the page just resized the canvas, which cleared it). */
  redraw(): boolean {
    if (!this.#attached) return false;
    try {
      return this.#deps.bridge.redraw();
    } catch {
      return false;
    }
  }

  /** Stops presenting, closes the ring and releases the canvas. */
  detach(): void {
    this.#stopWatch();
    this.#generation += 1;
    const { bridge } = this.#deps;
    if (this.#attached && this.#ring !== null) {
      try {
        bridge.stopPresenting();
        bridge.close();
      } catch {
        // The ring may already be gone; nothing left to release.
      }
    }
    // Also while an attach is still creating the device: the preload drops that one.
    if (this.#attached || this.#attaching) bridge.detachCanvas();
    this.#attaching = false;
    this.#attached = false;
    this.#ring = null;
    this.#set({ kind: "idle" });
  }

  /** Opens `ring` and presents it; the error message when that fails (`null` when it worked). */
  #open(ring: RingAnnouncement): string | null {
    const { bridge } = this.#deps;
    try {
      bridge.open(ring.name, ring.epoch);
      if (!this.#paused) {
        bridge.startPresenting({ latencyMs: this.#deps.latencyMs ?? PRESENT_LATENCY_MS });
      }
    } catch (e) {
      this.#ring = null;
      this.#lastError = message(e);
      return this.#lastError;
    }
    this.#ring = ring;
    this.#set({ kind: "presenting", adapter: this.#adapter });
    return null;
  }

  /** One failed attempt; the path gives up after {@link MAX_RECOVERIES} in a row. */
  #failed(text: string): void {
    this.#failures += 1;
    if (this.#failures >= MAX_RECOVERIES) this.#fail("ring", text);
    else this.#set({ kind: "recovering" });
  }

  #startWatch(): void {
    if (this.#watch !== null) return;
    this.#watch = this.#timers.setInterval(() => void this.#check(), WATCH_MS);
  }

  #stopWatch(): void {
    if (this.#watch !== null) this.#timers.clearInterval(this.#watch);
    this.#watch = null;
  }

  async #check(): Promise<void> {
    if (this.#recovering || !this.#attached) return;
    if (!this.#ended()) {
      this.#failures = 0;
      return;
    }
    this.#recovering = true;
    this.#set({ kind: "recovering" });
    try {
      const ring = await this.#deps.announce();
      if (!this.#attached) return;
      if (ring === null) {
        this.#fail("no-ring", "the engine stopped publishing preview frames");
        return;
      }
      // `setRing` (a new handshake) may have opened it meanwhile.
      if (this.#isCurrent(ring) && !this.#ended()) {
        this.#set({ kind: "presenting", adapter: this.#adapter });
        return;
      }
      const error = this.#open(ring);
      if (error !== null) this.#failed(error);
    } catch (e) {
      this.#failed(message(e));
    } finally {
      this.#recovering = false;
    }
  }

  #fail(reason: FailureReason, text: string): RingPreviewState {
    this.#stopWatch();
    if (this.#attached) {
      if (this.#ring !== null) {
        try {
          this.#deps.bridge.stopPresenting();
          this.#deps.bridge.close();
        } catch {
          // Already stopped or gone.
        }
      }
      this.#deps.bridge.detachCanvas();
      this.#attached = false;
    }
    this.#ring = null;
    this.#set({ kind: "failed", reason, message: text });
    return this.#state;
  }

  #set(state: RingPreviewState): void {
    this.#state = state;
    this.#deps.onState?.(state);
  }
}
