// SPDX-License-Identifier: GPL-3.0-or-later
// `TimelineView`: the framework-agnostic timeline. Owns the canvas, the viewport, the spatial
// layout, the media caches and the render loop; takes a read-only `TimelineModel` and a
// `ViewState`, never mutates either, and reports pointer events (with hit-test results) and
// user-driven view changes (wheel zoom, scroll) to its host. Interactions (select, move, trim,
// snap) are the state machine of `interaction.ts` (WP-1M.06), fed from `onPointer`.
import { MediaCaches, type MediaLoaders } from "./cache.ts";
import { nameRect } from "./geometry.ts";
import { type HitResult, hitTest } from "./hit-test.ts";
import { TimelineLayout } from "./layout.ts";
import {
  DEFAULT_VIEW_STATE,
  EMPTY_MODEL,
  type Rect,
  type TimelineModel,
  type ViewState,
} from "./model.ts";
import { TimelineRenderer } from "./renderer.ts";
import { readTheme, type Theme } from "./theme.ts";
import { FLICKS_PER_SECOND } from "./time.ts";
import {
  clampScroll,
  clampZoom,
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
  /** Frame scheduler (default `requestAnimationFrame`); tests pass a manual one. */
  readonly requestFrame?: (cb: () => void) => number;
  readonly cancelFrame?: (id: number) => void;
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
  private readonly vp: Viewport;
  private dpr = 1;
  private hoverItem = -1;
  private frameId = 0;
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
    this.requestFrame = options.requestFrame ?? ((cb) => requestAnimationFrame(cb));
    this.cancelFrame = options.cancelFrame ?? ((id) => cancelAnimationFrame(id));
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

  // ---- zoom and scroll --------------------------------------------------------------------

  /** Zoom by `factor` keeping the time under `anchorX` fixed (default: lane center). */
  zoomBy(factor: number, anchorX?: number): void {
    const ax = anchorX ?? this.vp.headerWidth + (this.vp.width - this.vp.headerWidth) / 2;
    const z = zoomAround(this.vp, factor, ax);
    this.userView({ pxPerSec: z.pxPerSec, scrollX: z.scrollX });
  }

  zoomTo(pxPerSec: number, anchorX?: number): void {
    this.zoomBy(clampZoom(pxPerSec) / this.vp.pxPerSec, anchorX);
  }

  /** Scroll by CSS pixels. */
  scrollBy(dx: number, dy = 0): void {
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
    this.userView({ pxPerSec: clampZoom((lane * FLICKS_PER_SECOND) / end), scrollX: 0 });
  }

  private userView(patch: Partial<ViewState>): void {
    this.setViewState(patch);
    for (const l of this.viewListeners) l(this.state);
  }

  // ---- canvas, theme, render loop ---------------------------------------------------------

  /** Size the canvas (CSS px) at the given device pixel ratio. */
  resize(cssWidth: number, cssHeight: number, dpr = 1): void {
    const w = Math.max(1, Math.floor(cssWidth));
    const h = Math.max(1, Math.floor(cssHeight));
    this.dpr = dpr;
    this.canvas.width = Math.max(1, Math.round(w * dpr));
    this.canvas.height = Math.max(1, Math.round(h * dpr));
    if (this.canvas.style) {
      this.canvas.style.width = `${w}px`;
      this.canvas.style.height = `${h}px`;
    }
    this.vp.width = w;
    this.vp.height = h;
    this.applyScroll();
    this.invalidate();
  }

  /** Re-read the design tokens (call on theme change; `bind()` does this automatically). */
  refreshTheme(): void {
    this.theme = this.fixedTheme ?? readTheme(this.canvas.ownerDocument.documentElement);
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
    if (this.frameId === 0) {
      this.frameId = this.requestFrame(() => {
        this.frameId = 0;
        this.flush();
      });
    }
  }

  /** Draw now if anything is dirty (the scheduled frame calls this). Returns whether it drew. */
  flush(): boolean {
    if (this.destroyed || (!this.dirtyFull && !this.dirty)) return false;
    if (this.vp.width === 0 || this.vp.height === 0) return false;
    const region = this.dirtyFull ? null : this.dirty;
    this.dirtyFull = false;
    this.dirty = null;
    this.renderer.draw(this.ctx, {
      layout: this.layout,
      vp: this.vp,
      theme: this.theme,
      fps: this.model.fps,
      dropFrame: this.model.dropFrame ?? false,
      view: this.state,
      caches: this.caches,
      dpr: this.dpr,
      hoverItem: this.hoverItem,
      region,
    });
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

  /**
   * Attach DOM listeners: pointer, wheel, `+`/`-` keys, `ResizeObserver` on the parent, theme
   * (`data-theme`) and device-pixel-ratio changes, font loading. Call `destroy()` to remove.
   */
  bind(): void {
    if (this.teardown.length > 0 || this.destroyed) return;
    const c = this.canvas;
    const doc = c.ownerDocument;
    const win = doc.defaultView;
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
        if (e.ctrlKey || e.metaKey) {
          this.zoomBy(Math.exp(-dy * 0.002), x);
        } else if (e.shiftKey || Math.abs(dx) > Math.abs(dy)) {
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
    if (parent && typeof ResizeObserver !== "undefined") {
      const ro = new ResizeObserver((entries) => {
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
