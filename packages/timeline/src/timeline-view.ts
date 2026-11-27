// SPDX-License-Identifier: GPL-3.0-or-later
// `TimelineView`: the framework-agnostic timeline. Owns the canvas, the viewport, the spatial
// layout, the media caches and the render loop; takes a read-only `TimelineModel` and a
// `ViewState`, never mutates either, and reports pointer events (with hit-test results) and
// user-driven view changes (wheel zoom, scroll) to its host. Interactions (select, move, trim,
// snap) are the state machine of `interaction/` (WP-2A.08), fed from `onPointer` (WP-2A.10).
import { MediaCaches, type MediaLoaders } from "./cache.ts";
import { nameRect } from "./geometry.ts";
import { type HitResult, hitTest } from "./hit-test.ts";
import { TimelineLayout } from "./layout.ts";
import {
  DEFAULT_DISPLAY,
  DEFAULT_VIEW_STATE,
  EMPTY_MODEL,
  type Rect,
  type TimelineDisplay,
  type TimelineMarker,
  type TimelineModel,
  type ViewState,
} from "./model.ts";
import { TimelineRenderer } from "./render/renderer.ts";
import { markerPinHeight } from "./render/ruler.ts";
import type { FrameInput } from "./render/state.ts";
import { readTheme, type Theme } from "./theme.ts";
import { FLICKS_PER_SECOND } from "./time.ts";
import {
  clampScroll,
  clampZoom,
  fitRange,
  timeToX,
  type Viewport,
  visibleTimeRange,
  xToTime,
  zoomAround,
} from "./viewport.ts";

/** A touch or pen held this long without moving is a right-click (WP-2H.12). */
export const LONG_PRESS_MS = 500;
/** Moving further than this cancels the long-press. */
export const LONG_PRESS_SLOP_PX = 8;

/** Zoom factor per wheel pixel (Ctrl/Cmd+wheel and pinch). */
export const WHEEL_ZOOM_PER_PX = 0.002;
/**
 * A Ctrl/Cmd+wheel step at least this large (CSS px) is a mouse-wheel notch and zooms smoothly
 * over a few frames; smaller steps (trackpad pinch, precision wheels) apply at once.
 */
export const WHEEL_NOTCH_PX = 50;
/** Time constant of the smooth zoom (ms): about 95% of the way after three of them. */
export const ZOOM_EASE_MS = 40;

export interface TimelinePointerEvent {
  /**
   * `cancel`: the browser took the pointer away (`pointercancel`); end any gesture.
   * `contextmenu`: the right button (or the platform's context-menu gesture) at this point; the
   * default menu is already suppressed. `longpress`: a touch or pen held still for
   * {@link LONG_PRESS_MS}; a `cancel` for the gesture it began precedes it.
   */
  readonly type:
    | "down"
    | "move"
    | "up"
    | "leave"
    | "dblclick"
    | "cancel"
    | "contextmenu"
    | "longpress";
  /** Canvas-relative CSS pixels. */
  readonly x: number;
  readonly y: number;
  readonly hit: HitResult;
  readonly button: number;
  readonly buttons: number;
  readonly shiftKey: boolean;
  readonly ctrlKey: boolean;
  readonly altKey: boolean;
  readonly metaKey: boolean;
  readonly original: MouseEvent;
}

export interface TimelineViewOptions {
  /** Fixed theme (tests, benchmarks). Default: resolved from the CSS variables, per theme change. */
  readonly theme?: Theme;
  /** Draw into this context instead of `canvas.getContext("2d")` (tests). */
  readonly context?: CanvasRenderingContext2D;
  readonly loaders?: MediaLoaders;
  /** Track-header column width, CSS px. Default: 4 x `space-12`. */
  readonly headerWidth?: number;
  readonly filmstripBytes?: number;
  readonly waveformBytes?: number;
  /**
   * Frame scheduler (default: the canvas's own window's `requestAnimationFrame`, so a timeline in
   * a detached window keeps drawing while the main window is hidden); tests pass a manual one.
   */
  readonly requestFrame?: (cb: () => void) => number;
  readonly cancelFrame?: (id: number) => void;
  /** Clock for the smooth zoom, ms (default `performance.now`); tests pass a fake one. */
  readonly now?: () => number;
  /**
   * Smooth (animated) wheel zoom. Default: on unless the user prefers reduced motion, in which
   * case every zoom step applies at once.
   */
  readonly smoothZoom?: boolean;
  /**
   * Middle-button drag: `builtin` (default) pans the view here; `host` reports it through
   * `onPointer` like any other button, for a host state machine that pans (WP-1M.06).
   */
  readonly pan?: "builtin" | "host";
}

type Unsubscribe = () => void;

export class TimelineView {
  readonly renderer = new TimelineRenderer();

  private readonly ctx: CanvasRenderingContext2D;
  private readonly requestFrame: (cb: () => void) => number;
  private readonly cancelFrame: (id: number) => void;
  private readonly fixedTheme: Theme | undefined;
  private readonly headerOverride: number | undefined;
  private theme: Theme;
  private model: TimelineModel = EMPTY_MODEL;
  private layout: TimelineLayout;
  private caches: MediaCaches;
  private loaders: MediaLoaders;
  private state: ViewState = DEFAULT_VIEW_STATE;
  private display: TimelineDisplay = DEFAULT_DISPLAY;
  private readonly vp: Viewport;
  /** A smooth zoom in progress: eases `current` toward `target`, keeping `anchorT` at `anchorX`. */
  private zoomAnim: {
    target: number;
    current: number;
    anchorX: number;
    anchorT: number;
    last: number;
  } | null = null;
  private readonly now: () => number;
  private readonly frameInput: { -readonly [K in keyof FrameInput]-?: FrameInput[K] };
  private dpr = 1;
  private hoverItem = -1;
  private frameId = 0;
  /** The window the pending frame was asked of (the canvas's when it was asked). */
  private frameWindow: Window | null = null;
  /** The window the DOM listeners are bound in. */
  private boundWindow: Window | null = null;
  /** Stops waiting for a stylesheet to read the tokens from (`refreshTheme`). */
  private themeRetry: (() => void) | null = null;
  private dirtyFull = true;
  private dirty: Rect | null = null;
  private destroyed = false;
  private readonly scratch: number[] = [];
  private readonly pointerListeners = new Set<(e: TimelinePointerEvent) => void>();
  private readonly viewListeners = new Set<(s: ViewState) => void>();
  private teardown: Array<() => void> = [];
  private panning: { x: number; scrollX: number; y: number; scrollY: number } | null = null;
  private press: { x: number; y: number; timer: ReturnType<typeof setTimeout> } | null = null;
  /** A long-press opened the menu: the `contextmenu` event the platform adds is not a second one. */
  private longPressed = false;

  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly options: TimelineViewOptions = {},
  ) {
    const ctx = options.context ?? canvas.getContext("2d", { alpha: false });
    if (!ctx) throw new Error("@cutweave/timeline: Canvas2D unavailable");
    this.ctx = ctx;
    this.requestFrame =
      options.requestFrame ??
      ((cb) => {
        const win = this.canvasWindow();
        this.frameWindow = win;
        return win.requestAnimationFrame(cb);
      });
    this.cancelFrame =
      options.cancelFrame ??
      ((id) => {
        try {
          (this.frameWindow ?? this.canvasWindow()).cancelAnimationFrame(id);
        } catch {
          // The window it was asked of has closed, and its frames with it.
        }
      });
    this.now = options.now ?? (() => performance.now());
    this.fixedTheme = options.theme;
    this.headerOverride = options.headerWidth;
    this.theme = options.theme ?? readTheme(canvas.ownerDocument.documentElement);
    this.loaders = options.loaders ?? {};
    this.caches = this.makeCaches();
    this.vp = {
      pxPerSec: this.state.pxPerSec,
      scrollX: 0,
      scrollY: 0,
      width: 0,
      height: 0,
      headerWidth: this.headerOverride ?? 4 * this.theme.space12,
      rulerHeight: this.theme.controlMd,
    };
    this.layout = this.buildLayout();
    this.frameInput = {
      layout: this.layout,
      vp: this.vp,
      theme: this.theme,
      fps: this.model.fps,
      dropFrame: false,
      view: this.state,
      caches: this.caches,
      dpr: this.dpr,
      hoverItem: -1,
      region: null,
      display: this.display,
    };
  }

  // ---- model and state --------------------------------------------------------------------

  /** Replace the projection. Rebuilds the spatial index (O(n log n); 5,000 items take a few ms). */
  setModel(model: TimelineModel): void {
    this.model = model;
    this.layout = this.buildLayout();
    this.hoverItem = -1;
    this.applyScroll();
    this.invalidate();
  }

  /** Apply a partial view state. Invalidates only what changed (playhead and marquee are regional). */
  setViewState(patch: Partial<ViewState>): void {
    const prev = this.state;
    const next: ViewState = { ...prev, ...patch };
    this.vp.pxPerSec = clampZoom(next.pxPerSec);
    this.vp.scrollX = next.scrollX;
    this.vp.scrollY = next.scrollY;
    this.applyScroll();
    const merged: ViewState = {
      ...next,
      pxPerSec: this.vp.pxPerSec,
      scrollX: this.vp.scrollX,
      scrollY: this.vp.scrollY,
    };
    this.state = merged;

    if (
      merged.pxPerSec !== prev.pxPerSec ||
      merged.scrollX !== prev.scrollX ||
      merged.scrollY !== prev.scrollY ||
      merged.selection !== prev.selection ||
      merged.snapGuides !== prev.snapGuides ||
      merged.range !== prev.range
    ) {
      this.invalidate();
      return;
    }
    if (merged.playhead !== prev.playhead) {
      this.invalidate(this.playheadStrip(prev.playhead));
      this.invalidate(this.playheadStrip(merged.playhead));
    }
    if (merged.marquee !== prev.marquee) {
      for (const m of [prev.marquee, merged.marquee]) {
        if (m) this.invalidate({ x: m.x - 2, y: m.y - 2, w: m.w + 4, h: m.h + 4 });
      }
    }
  }

  getViewState(): ViewState {
    return this.state;
  }

  getModel(): TimelineModel {
    return this.model;
  }

  /** Apply display preferences (how time shows); redraws when one changed. */
  setDisplay(patch: Partial<TimelineDisplay>): void {
    const next: TimelineDisplay = { ...this.display, ...patch };
    const d = this.display;
    if (
      next.timeFormat === d.timeFormat &&
      next.dropFrame === d.dropFrame &&
      next.thumbnails === d.thumbnails &&
      next.waveforms === d.waveforms &&
      next.wheel === d.wheel
    ) {
      return;
    }
    this.display = next;
    this.invalidate();
  }

  getDisplay(): TimelineDisplay {
    return this.display;
  }

  /** Replace the media loaders (clears the caches). */
  setLoaders(loaders: MediaLoaders): void {
    this.loaders = loaders;
    this.caches.dispose();
    this.caches = this.makeCaches();
    this.invalidate();
  }

  // ---- geometry API (for 1M.06) -----------------------------------------------------------

  /** Canvas x (CSS px) of time `t` flicks. */
  timeToX(t: number): number {
    return timeToX(this.vp, t);
  }

  /** Time (integer flicks) at canvas x. */
  xToTime(x: number): number {
    return xToTime(this.vp, x);
  }

  /** Visible lane time range, flicks. */
  visibleRange(): { t0: number; t1: number } {
    return visibleTimeRange(this.vp);
  }

  /** What is under canvas point `(x, y)` (CSS px). */
  hitTest(x: number, y: number): HitResult {
    return hitTest(
      {
        layout: this.layout,
        vp: this.vp,
        playhead: this.state.playhead,
        theme: this.theme,
        flagHalfWidth: this.renderer.flagHalfWidth || 4 * this.theme.space4,
        scratch: this.scratch,
      },
      x,
      y,
    );
  }

  /** Canvas-space rect of a track's name in its header (the rename input goes here), or null. */
  trackNameRect(trackId: string): Rect | null {
    const ti = this.layout.model.tracks.findIndex((t) => t.id === trackId);
    if (ti < 0) return null;
    const laneTop = this.vp.rulerHeight + (this.layout.trackTop[ti] ?? 0) - this.vp.scrollY;
    return nameRect(this.vp, this.theme, laneTop, this.layout.trackHeight[ti] ?? 0);
  }

  /** Canvas-space bounding box of an item, or null when unknown. */
  itemRect(itemId: string): Rect | null {
    const i = this.layout.itemIndexById.get(itemId);
    return i === undefined ? null : this.layout.itemRect(i, this.vp);
  }

  /** Ids of items whose clip box intersects `rect` (canvas CSS px): marquee selection. */
  itemsInRect(rect: Rect): string[] {
    const { layout, vp } = this;
    const out: string[] = [];
    const t0 = xToTime(vp, rect.x);
    const t1 = xToTime(vp, rect.x + rect.w);
    const buf: number[] = [];
    for (let ti = 0; ti < layout.trackCount; ti++) {
      const top = vp.rulerHeight + (layout.trackTop[ti] ?? 0) - vp.scrollY + layout.pad;
      if (top + layout.clipHeight(ti) <= rect.y || top >= rect.y + rect.h) continue;
      for (const i of layout.query(ti, t0, t1, buf)) {
        const it = layout.items[i];
        if (it) out.push(it.id);
      }
    }
    return out;
  }

  /** Track index and id at canvas y, or null over the ruler / below the last track. */
  trackAtY(y: number): { id: string; index: number } | null {
    const ti = this.layout.trackAtContentY(y - this.vp.rulerHeight + this.vp.scrollY);
    const t = this.model.tracks[ti];
    return ti >= 0 && t ? { id: t.id, index: ti } : null;
  }

  /**
   * The sequence marker drawn under canvas point `(x, y)` (its pin in the ruler, or the bar of a
   * range marker), the nearest one when several are within reach; null elsewhere. For the
   * interaction layer (WP-2A.10: double-click or right-click a marker).
   */
  markerAt(x: number, y: number): TimelineMarker | null {
    const { vp, theme: th, layout } = this;
    const baseY = vp.rulerHeight - th.borderThin;
    if (x < vp.headerWidth || y < baseY - markerPinHeight(th) || y > vp.rulerHeight) return null;
    const reach = th.space2 / 2 + 1;
    const t = xToTime(vp, x);
    const k = vp.pxPerSec / FLICKS_PER_SECOND;
    let best: TimelineMarker | null = null;
    let bestDx = Number.POSITIVE_INFINITY;
    for (let n = layout.firstMarkerFrom(t - reach / k); n < layout.markers.length; n++) {
      const m = layout.markers[n] as TimelineMarker;
      const mx = timeToX(vp, m.t);
      if (mx > x + reach) break;
      const dx = Math.abs(mx - x);
      const inBar = (m.duration ?? 0) > 0 && x >= mx && x <= timeToX(vp, m.t + (m.duration ?? 0));
      if (dx <= reach && dx < bestDx) {
        best = m;
        bestDx = dx;
      } else if (inBar && best === null) {
        best = m;
      }
    }
    return best;
  }

  // ---- zoom and scroll --------------------------------------------------------------------

  /** Zoom by `factor` keeping the time under `anchorX` fixed (default: lane center). Instant. */
  zoomBy(factor: number, anchorX?: number): void {
    this.zoomAnim = null;
    const ax = anchorX ?? this.vp.headerWidth + (this.vp.width - this.vp.headerWidth) / 2;
    const z = zoomAround(this.vp, factor, ax);
    this.userView({ pxPerSec: z.pxPerSec, scrollX: z.scrollX });
  }

  zoomTo(pxPerSec: number, anchorX?: number): void {
    this.zoomBy(clampZoom(pxPerSec) / this.vp.pxPerSec, anchorX);
  }

  /**
   * Zoom smoothly toward `factor` times the current target (a wheel notch), keeping the time
   * under `anchorX` fixed: the zoom eases there over a few frames, and further steps retarget it.
   * Applies at once when smooth zoom is off (reduced motion).
   */
  zoomSmooth(factor: number, anchorX: number): void {
    if (!this.smoothZoom()) {
      this.zoomBy(factor, anchorX);
      return;
    }
    const k = this.vp.pxPerSec / FLICKS_PER_SECOND;
    const anchorT = this.vp.scrollX + (anchorX - this.vp.headerWidth) / k;
    const a = this.zoomAnim;
    const from = a?.current ?? this.vp.pxPerSec;
    const target = clampZoom((a?.target ?? this.vp.pxPerSec) * factor);
    this.zoomAnim = { target, current: from, anchorX, anchorT, last: this.now() };
    this.invalidate();
  }

  /** Whether a smooth zoom is still easing toward its target. */
  get zooming(): boolean {
    return this.zoomAnim !== null;
  }

  /** Show `[t0, t1]` (flicks) across the lane with a small margin (zoom to selection). */
  zoomToRange(t0: number, t1: number): void {
    this.zoomAnim = null;
    this.userView(fitRange(this.vp, Math.min(t0, t1), Math.max(t0, t1), this.theme.space4));
  }

  /** Scroll by CSS pixels. */
  scrollBy(dx: number, dy = 0): void {
    // A pan during a smooth zoom carries the zoom's anchor along, so both compose.
    const a = this.zoomAnim;
    if (a !== null) a.anchorT += dx * (FLICKS_PER_SECOND / this.vp.pxPerSec);
    this.userView({
      scrollX: this.vp.scrollX + dx * (FLICKS_PER_SECOND / this.vp.pxPerSec),
      scrollY: this.vp.scrollY + dy,
    });
  }

  /** Fit the whole content into the lane area (with a small margin). */
  zoomToFit(): void {
    const end = this.layout.contentEnd;
    const lane = Math.max(1, this.vp.width - this.vp.headerWidth - 2 * this.theme.space4);
    if (end <= 0) return;
    this.zoomAnim = null;
    this.userView({ pxPerSec: clampZoom((lane * FLICKS_PER_SECOND) / end), scrollX: 0 });
  }

  /** One step of the smooth zoom (called at the start of a frame, before drawing). */
  private stepZoom(): void {
    const a = this.zoomAnim;
    if (a === null) return;
    const now = this.now();
    const dt = Math.min(64, Math.max(0, now - a.last));
    a.last = now;
    const ease = 1 - Math.exp(-dt / ZOOM_EASE_MS);
    let next = a.current * (a.target / a.current) ** ease;
    if (Math.abs(Math.log(a.target / next)) < 0.002) {
      next = a.target;
      this.zoomAnim = null;
    }
    a.current = next;
    const scrollX = Math.round(
      a.anchorT - (a.anchorX - this.vp.headerWidth) / (next / FLICKS_PER_SECOND),
    );
    this.userView({ pxPerSec: next, scrollX });
  }

  private smoothZoom(): boolean {
    if (this.options.smoothZoom !== undefined) return this.options.smoothZoom;
    const doc = this.canvas.ownerDocument;
    // Settings > Appearance > Reduce motion (`data-motion` on the root) overrides the system.
    const choice = doc?.documentElement?.getAttribute("data-motion");
    if (choice === "reduce") return false;
    if (choice === "full") return true;
    return !(doc?.defaultView?.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false);
  }

  private userView(patch: Partial<ViewState>): void {
    this.setViewState(patch);
    for (const l of this.viewListeners) l(this.state);
  }

  // ---- canvas, theme, render loop ---------------------------------------------------------

  /**
   * Size the canvas (CSS px) at the given device pixel ratio, and draw it at once (#163).
   *
   * Setting the canvas size clears it, and the `ResizeObserver` that calls this runs between
   * layout and paint: drawing on the next frame would show the cleared canvas for every frame
   * of a splitter drag. So a resize draws synchronously, and a size that did not change touches
   * nothing (assigning the same size would still clear the canvas).
   */
  resize(cssWidth: number, cssHeight: number, dpr = 1): void {
    const w = Math.max(1, Math.floor(cssWidth));
    const h = Math.max(1, Math.floor(cssHeight));
    const bw = Math.max(1, Math.round(w * dpr));
    const bh = Math.max(1, Math.round(h * dpr));
    const backingChanged = this.canvas.width !== bw || this.canvas.height !== bh;
    const cssChanged = this.vp.width !== w || this.vp.height !== h;
    if (!backingChanged && !cssChanged && this.dpr === dpr) return;
    this.dpr = dpr;
    if (backingChanged) {
      this.canvas.width = bw;
      this.canvas.height = bh;
    }
    if (cssChanged && this.canvas.style) {
      this.canvas.style.width = `${w}px`;
      this.canvas.style.height = `${h}px`;
    }
    this.vp.width = w;
    this.vp.height = h;
    this.applyScroll();
    this.dirtyFull = true;
    this.dirty = null;
    this.flush();
  }

  /**
   * Re-read the design tokens (call on theme change; `bind()` does this automatically). A canvas
   * that just moved into another window's document may find its stylesheets still loading (the
   * window copies them as links): it keeps the tokens it has and reads them again once a
   * stylesheet there has loaded.
   */
  refreshTheme(): void {
    if (this.fixedTheme === undefined) {
      const doc = this.canvas.ownerDocument;
      try {
        this.theme = readTheme(doc.documentElement);
      } catch {
        this.retryThemeOnLoad(doc);
        return;
      }
    } else {
      this.theme = this.fixedTheme;
    }
    this.vp.rulerHeight = this.theme.controlMd;
    this.vp.headerWidth = this.headerOverride ?? 4 * this.theme.space12;
    this.layout = this.buildLayout();
    this.renderer.resetMetrics();
    this.applyScroll();
    this.invalidate();
  }

  /** Mark a canvas region (CSS px) dirty, or everything when omitted. Coalesced per frame. */
  invalidate(region?: Rect): void {
    if (this.destroyed) return;
    if (!region) this.dirtyFull = true;
    else if (!this.dirtyFull) {
      const d = this.dirty;
      if (!d) this.dirty = region;
      else {
        const x0 = Math.min(d.x, region.x);
        const y0 = Math.min(d.y, region.y);
        const x1 = Math.max(d.x + d.w, region.x + region.w);
        const y1 = Math.max(d.y + d.h, region.y + region.h);
        this.dirty = { x: x0, y: y0, w: x1 - x0, h: y1 - y0 };
      }
    }
    // The canvas moved to another window (its panel was detached or docked back): a frame asked
    // of the old window may never come, so ask the new one.
    if (this.frameId !== 0 && this.frameWindow !== null) {
      if (this.frameWindow !== this.canvasWindow()) {
        this.cancelFrame(this.frameId);
        this.frameId = 0;
      }
    }
    if (this.frameId === 0) this.frameId = this.requestFrame(this.onFrame);
  }

  /** The scheduled frame: advance a smooth zoom (which asks for the next frame), then draw. */
  private readonly onFrame = (): void => {
    this.frameId = 0;
    this.stepZoom();
    this.flush();
  };

  /** Draw now if anything is dirty (the scheduled frame calls this). Returns whether it drew. */
  flush(): boolean {
    if (this.destroyed || (!this.dirtyFull && !this.dirty)) return false;
    if (this.vp.width === 0 || this.vp.height === 0) return false;
    const region = this.dirtyFull ? null : this.dirty;
    this.dirtyFull = false;
    this.dirty = null;
    // One input object for every frame, refreshed in place (no allocation per frame).
    const f = this.frameInput;
    f.layout = this.layout;
    f.vp = this.vp;
    f.theme = this.theme;
    f.fps = this.model.fps;
    f.dropFrame = this.display.dropFrame ?? this.model.dropFrame ?? false;
    f.view = this.state;
    f.caches = this.caches;
    f.dpr = this.dpr;
    f.hoverItem = this.hoverItem;
    f.region = region;
    f.display = this.display;
    this.renderer.draw(this.ctx, f);
    return true;
  }

  /** Draw counters of the last frame (benchmarks, tests). */
  get stats(): TimelineRenderer["stats"] {
    return this.renderer.stats;
  }

  get cacheStats(): { filmstripBytes: number; tiles: number; peaks: number; pending: number } {
    return {
      filmstripBytes: this.caches.filmstrip.byteSize,
      tiles: this.caches.filmstrip.size,
      peaks: this.caches.waveform.size,
      pending: this.caches.filmstrip.pending + this.caches.waveform.pending,
    };
  }

  // ---- events -----------------------------------------------------------------------------

  /** Subscribe to pointer events with their hit-test result (consumed by WP-1M.06). */
  onPointer(cb: (e: TimelinePointerEvent) => void): Unsubscribe {
    this.pointerListeners.add(cb);
    return () => this.pointerListeners.delete(cb);
  }

  /** Subscribe to view changes the user caused (wheel, zoom keys); host syncs its `viewStore`. */
  onViewChange(cb: (s: ViewState) => void): Unsubscribe {
    this.viewListeners.add(cb);
    return () => this.viewListeners.delete(cb);
  }

  /** The tokens could not be read yet: read them again when a stylesheet of `doc` has loaded. */
  private retryThemeOnLoad(doc: Document): void {
    if (this.themeRetry !== null || this.destroyed) return;
    const onLoad = (): void => {
      this.themeRetry?.();
      this.refreshTheme();
    };
    // A `<link>`'s load does not bubble; the document hears it while capturing.
    doc.addEventListener("load", onLoad, true);
    this.themeRetry = () => {
      doc.removeEventListener("load", onLoad, true);
      this.themeRetry = null;
    };
  }

  /** The window the canvas is in now (a detached panel's window has its own). */
  private canvasWindow(): Window & typeof globalThis {
    return (this.canvas.ownerDocument.defaultView ?? globalThis) as Window & typeof globalThis;
  }

  /**
   * The canvas moved to another window (its panel was detached into its own window, or docked
   * back): observe its size, theme, fonts and display density there, take its frames from there,
   * and redraw at that window's density. Nothing happens when it is still where it was bound.
   */
  rehome(): void {
    if (this.destroyed || this.teardown.length === 0) return;
    if (this.boundWindow === this.canvasWindow()) return;
    for (const t of this.teardown) t();
    this.teardown = [];
    this.themeRetry?.();
    this.bind();
    if (!this.fixedTheme) this.refreshTheme();
    this.invalidate();
  }

  /**
   * Attach DOM listeners: pointer, wheel, `+`/`-` keys, `ResizeObserver` on the parent, theme
   * (`data-theme`) and device-pixel-ratio changes, font loading. Call `destroy()` to remove. They
   * are the canvas's own window's; `rehome()` moves them when the canvas changes window.
   */
  bind(): void {
    if (this.teardown.length > 0 || this.destroyed) return;
    const c = this.canvas;
    const doc = c.ownerDocument;
    const win = doc.defaultView;
    this.boundWindow = win;
    const on = <K extends keyof HTMLElementEventMap>(
      type: K,
      fn: (e: HTMLElementEventMap[K]) => void,
      opts?: AddEventListenerOptions,
    ): void => {
      c.addEventListener(type, fn, opts);
      this.teardown.push(() => c.removeEventListener(type, fn, opts));
    };
    const local = (e: MouseEvent): { x: number; y: number } => {
      const r = c.getBoundingClientRect();
      return { x: e.clientX - r.left, y: e.clientY - r.top };
    };
    const emit = (type: TimelinePointerEvent["type"], e: MouseEvent): void => {
      const { x, y } = local(e);
      const hit = this.hitTest(x, y);
      if (type === "move") this.setHover(hit.itemIndex);
      if (type !== "leave") c.style.cursor = cursorFor(hit);
      const ev: TimelinePointerEvent = {
        type,
        x,
        y,
        hit,
        button: e.button,
        buttons: e.buttons,
        shiftKey: e.shiftKey,
        ctrlKey: e.ctrlKey,
        altKey: e.altKey,
        metaKey: e.metaKey,
        original: e,
      };
      for (const l of this.pointerListeners) l(ev);
    };

    const endPress = (): void => {
      if (this.press !== null) clearTimeout(this.press.timer);
      this.press = null;
    };
    this.teardown.push(endPress);

    on("pointerdown", (e) => {
      this.longPressed = false;
      if (e.button === 1) e.preventDefault(); // no autoscroll cursor
      if (e.button === 1 && this.options.pan !== "host") {
        // Middle button drags the timeline (built-in pan); everything else goes to the host.
        c.setPointerCapture(e.pointerId);
        this.panning = {
          x: e.clientX,
          scrollX: this.vp.scrollX,
          y: e.clientY,
          scrollY: this.vp.scrollY,
        };
        return;
      }
      c.setPointerCapture(e.pointerId);
      emit("down", e);
      if (e.pointerType === "touch" || e.pointerType === "pen") {
        endPress();
        const timer = setTimeout(() => {
          this.press = null;
          this.longPressed = true;
          emit("cancel", e); // the gesture it began ends, rolled back
          emit("longpress", e);
        }, LONG_PRESS_MS);
        this.press = { x: e.clientX, y: e.clientY, timer };
      }
    });
    on("pointermove", (e) => {
      if (this.panning) {
        this.zoomAnim = null;
        const k = FLICKS_PER_SECOND / this.vp.pxPerSec;
        this.userView({
          scrollX: this.panning.scrollX - (e.clientX - this.panning.x) * k,
          scrollY: this.panning.scrollY - (e.clientY - this.panning.y),
        });
        return;
      }
      if (
        this.press !== null &&
        Math.hypot(e.clientX - this.press.x, e.clientY - this.press.y) > LONG_PRESS_SLOP_PX
      ) {
        endPress();
      }
      emit("move", e);
    });
    on("pointerup", (e) => {
      endPress();
      if (this.panning) {
        this.panning = null;
        return;
      }
      emit("up", e);
    });
    on("pointerleave", (e) => {
      this.setHover(-1);
      emit("leave", e);
    });
    on("pointercancel", (e) => {
      endPress();
      this.panning = null;
      emit("cancel", e);
    });
    on("dblclick", (e) => emit("dblclick", e));
    on("contextmenu", (e) => {
      // Never the browser's menu: the host draws its own for what is under the pointer.
      e.preventDefault();
      if (this.longPressed) return;
      emit("contextmenu", e);
    });
    on(
      "wheel",
      (e) => {
        e.preventDefault();
        const { x } = local(e);
        const unit = e.deltaMode === 1 ? 16 : e.deltaMode === 2 ? this.vp.height : 1;
        const dx = e.deltaX * unit;
        const dy = e.deltaY * unit;
        // The Timeline preference "Mouse wheel": it scrolls and Ctrl/Cmd+wheel zooms, or the
        // other way round. A sideways swipe or Shift+wheel always scrolls along the time.
        const modified = e.ctrlKey || e.metaKey;
        const sideways = e.shiftKey || Math.abs(dx) > Math.abs(dy);
        const zooms = this.display.wheel === "zoom" ? !modified && !sideways : modified;
        if (zooms) {
          // Zoom anchored at the pointer: a wheel notch eases there, a pinch follows the fingers.
          const factor = Math.exp(-dy * WHEEL_ZOOM_PER_PX);
          if (Math.abs(dy) >= WHEEL_NOTCH_PX) this.zoomSmooth(factor, x);
          else this.zoomBy(factor, x);
        } else if (sideways) {
          this.scrollBy(dx || dy);
        } else if (this.layout.totalHeight > this.vp.height - this.vp.rulerHeight) {
          this.scrollBy(0, dy);
        } else {
          this.scrollBy(dy);
        }
      },
      { passive: false },
    );
    on("keydown", (e) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      if (e.key === "+" || e.key === "=") {
        e.preventDefault();
        this.zoomBy(1.25);
      } else if (e.key === "-" || e.key === "_") {
        e.preventDefault();
        this.zoomBy(0.8);
      }
    });

    const parent = c.parentElement;
    // The canvas's own window's observer: one from another window fires unreliably for it.
    const Observer = win?.ResizeObserver ?? globalThis.ResizeObserver;
    if (parent && Observer !== undefined) {
      const ro = new Observer((entries) => {
        const box = entries[entries.length - 1]?.contentRect;
        if (box) this.resize(box.width, box.height, win?.devicePixelRatio ?? 1);
      });
      ro.observe(parent);
      this.teardown.push(() => ro.disconnect());
      const r = parent.getBoundingClientRect();
      if (r.width > 0 && r.height > 0) this.resize(r.width, r.height, win?.devicePixelRatio ?? 1);
    }

    if (!this.fixedTheme && typeof MutationObserver !== "undefined") {
      const mo = new MutationObserver(() => this.refreshTheme());
      mo.observe(doc.documentElement, {
        attributes: true,
        attributeFilter: ["data-theme", "data-density", "class"],
      });
      this.teardown.push(() => mo.disconnect());
    }
    const fonts = doc.fonts;
    if (fonts) {
      const fn = (): void => {
        this.renderer.resetMetrics();
        this.invalidate();
      };
      fonts.addEventListener("loadingdone", fn);
      this.teardown.push(() => fonts.removeEventListener("loadingdone", fn));
    }
    if (win?.matchMedia) {
      // Re-render at the new density when the window moves to another display.
      let mq: MediaQueryList | null = null;
      const arm = (): void => {
        mq?.removeEventListener("change", onChange);
        mq = win.matchMedia(`(resolution: ${win.devicePixelRatio}dppx)`);
        mq.addEventListener("change", onChange);
      };
      const onChange = (): void => {
        this.resize(this.vp.width, this.vp.height, win.devicePixelRatio);
        arm();
      };
      arm();
      this.teardown.push(() => mq?.removeEventListener("change", onChange));
    }
  }

  destroy(): void {
    this.destroyed = true;
    this.zoomAnim = null;
    this.themeRetry?.();
    if (this.frameId !== 0) this.cancelFrame(this.frameId);
    this.frameId = 0;
    for (const t of this.teardown) t();
    this.teardown = [];
    this.pointerListeners.clear();
    this.viewListeners.clear();
    this.caches.dispose();
  }

  // ---- internals --------------------------------------------------------------------------

  private makeCaches(): MediaCaches {
    return new MediaCaches({
      loaders: this.loaders,
      onLoaded: () => this.invalidate(),
      ...(this.options.filmstripBytes !== undefined && {
        filmstripBytes: this.options.filmstripBytes,
      }),
      ...(this.options.waveformBytes !== undefined && {
        waveformBytes: this.options.waveformBytes,
      }),
    });
  }

  private buildLayout(): TimelineLayout {
    const th = this.theme;
    return new TimelineLayout(this.model, {
      sizes: { sm: th.trackSm, md: th.trackMd, lg: th.trackLg },
      pad: th.space1,
    });
  }

  private applyScroll(): void {
    const s = clampScroll(this.vp, this.layout.contentEnd, this.layout.totalHeight);
    this.vp.scrollX = s.scrollX;
    this.vp.scrollY = s.scrollY;
    if (this.state.scrollX !== s.scrollX || this.state.scrollY !== s.scrollY) {
      this.state = { ...this.state, scrollX: s.scrollX, scrollY: s.scrollY };
    }
  }

  private playheadStrip(t: number): Rect {
    const { x0, x1 } = this.renderer.playheadSpan(timeToX(this.vp, t), this.vp, this.theme);
    return { x: x0, y: 0, w: x1 - x0, h: this.vp.height };
  }

  private setHover(item: number): void {
    if (item === this.hoverItem) return;
    for (const i of [this.hoverItem, item]) {
      const r = i >= 0 ? this.layout.itemRect(i, this.vp) : null;
      if (r) this.invalidate({ x: r.x - 2, y: r.y - 2, w: r.w + 4, h: r.h + 4 });
    }
    this.hoverItem = item;
  }
}

function cursorFor(hit: HitResult): string {
  switch (hit.zone) {
    case "left-edge":
    case "right-edge":
      return "ew-resize";
    case "header-toggle":
    case "keyframe":
      return "pointer";
    case "header-name":
      return "text";
    default:
      return "default";
  }
}
