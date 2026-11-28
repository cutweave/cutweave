// SPDX-License-Identifier: GPL-3.0-or-later
// The graph editor's canvas (WP-2C.07), framework-free: it holds the curves, the selection, the
// viewport and the playhead, draws them (at most once per animation frame, only when something
// changed) and turns pointer and key input into events. Nothing here is a React element, so a
// thousand keys cost a thousand small fills, not a thousand components, and new data redraws the
// canvas without rendering anything around it.
import type { CurveValue } from "../curve-editor/CurveEditor.tsx";
import {
  type BezierNumbers,
  type Box,
  boxOf,
  constrainAxis,
  denormalizeDelta,
  type Extent,
  fitExtent,
  type GraphViewport,
  type HandleSide,
  handleNumbers,
  handlePoint,
  keysInBox,
  lowerBound,
  nearestKey,
  niceStep,
  normalize,
  oppositeOfIn,
  oppositeOfOut,
  type PlotRect,
  panBy,
  type Segment,
  type TangentMode,
  tAt,
  ticks,
  timeStep,
  vAt,
  valueRange,
  xOf,
  yOf,
  zoomAbout,
} from "./geometry.ts";
import { type GraphTheme, readGraphTheme } from "./theme.ts";

/** One value-over-time curve: a property, or one component of a two-dimensional one. */
export interface GraphCurve {
  readonly id: string;
  readonly label: string;
  /** Index into the theme's curve colours. */
  readonly color: number;
  /** Key times, ascending, in the graph's time units. */
  readonly times: ArrayLike<number>;
  /** Key values, one per time. */
  readonly values: ArrayLike<number>;
  /** How the segment starting at each key moves to the next key. */
  readonly interps: readonly CurveValue[];
  /**
   * Writes the curve's value at `n` evenly spaced times from `t0` to `t1` into `out` (the
   * engine's own math, so the curve drawn is the curve rendered).
   */
  sample(t0: number, t1: number, n: number, out: Float64Array): void;
  /** The curve cannot be edited (a locked track). */
  readonly locked?: boolean;
}

/** A key of a curve, by its index in the curve's arrays. */
export interface GraphPointRef {
  readonly curve: string;
  readonly index: number;
}

export type GraphEdit =
  /**
   * The selected keys moved by `dt` in time; the keys of curve `id` moved by `dv[id]` in value
   * (curves without an entry keep their values).
   */
  | {
      readonly type: "move";
      readonly grabbed: GraphPointRef;
      readonly dt: number;
      readonly dv: Readonly<Record<string, number>>;
    }
  /**
   * A handle of key `index` dragged: `out` is (`x1`, `y1`) of the segment starting at the key,
   * `in` is (`x2`, `y2`) of the segment ending at it. `opposite` is the handle on the key's
   * other side when the tangent mode moved it too.
   */
  | {
      readonly type: "tangent";
      readonly curve: string;
      readonly index: number;
      readonly side: HandleSide;
      readonly x: number;
      readonly y: number;
      readonly opposite: { readonly x: number; readonly y: number } | null;
    };

export type GraphAction =
  | "delete"
  | "selectAll"
  | "copy"
  | "paste"
  | "easing"
  | "previousKey"
  | "nextKey"
  | "clear";

export interface GraphSnap {
  /** The time to use. */
  readonly t: number;
  /** A time to draw a guide at, or null. */
  readonly guide: number | null;
}

/** What the canvas tells its host. Every member is optional. */
export interface GraphEvents {
  /** Keys picked by a click or a box. */
  select?(points: readonly GraphPointRef[], mode: "replace" | "toggle" | "add"): void;
  /**
   * A drag of keys or of a handle: `update` with every new position (the first one opens the
   * gesture), then `commit` or `cancel` once.
   */
  gesture?(phase: "update" | "commit" | "cancel", edit: GraphEdit | null): void;
  /** A double-click: a key at time `t` of the curve. */
  add?(curve: string, t: number): void;
  /** The arrow keys: the selected keys move by frames and by value steps. */
  nudge?(frames: number, steps: number): void;
  action?(name: GraphAction): void;
  /** The time ruler was clicked or dragged: the playhead goes there. */
  scrub?(t: number): void;
  /** The viewport changed by the person's hand (a pan, a zoom, a fit). */
  viewport?(viewport: GraphViewport): void;
}

export interface GraphOptions {
  /** One second and one frame, in time units (ruler steps; a nudge is one frame). */
  readonly second: number;
  readonly frame: number;
  formatTime(t: number): string;
  formatValue(v: number): string;
  /** Snaps a dragged key's time (`unitsPerPx` is the zoom). Absent: no snapping. */
  snap?(t: number, unitsPerPx: number): GraphSnap;
}

/** Frame scheduling of the canvas's own window (tests pass their own). */
export interface GraphFrames {
  request(callback: () => void): number;
  cancel(handle: number): void;
}

type Drag =
  | { readonly kind: "pan"; x: number; y: number }
  | { readonly kind: "scrub" }
  | {
      readonly kind: "marquee";
      readonly x0: number;
      readonly y0: number;
      x: number;
      y: number;
      readonly add: boolean;
    }
  | {
      readonly kind: "move";
      readonly x0: number;
      readonly y0: number;
      readonly grabbed: GraphPointRef;
      readonly origT: number;
      readonly ranges: ReadonlyMap<string, { lo: number; hi: number }>;
      readonly curves: readonly string[];
      /** Pressed with Shift or Ctrl on a selected key: a click without a drag takes it out. */
      readonly toggleOnClick: boolean;
      started: boolean;
      last: GraphEdit | null;
    }
  | {
      readonly kind: "tangent";
      readonly curve: string;
      readonly index: number;
      readonly side: HandleSide;
      readonly next: Segment | null;
      readonly prev: Segment | null;
      readonly nextB: BezierNumbers | null;
      readonly prevB: BezierNumbers | null;
      readonly range: { lo: number; hi: number };
      started: boolean;
      last: GraphEdit | null;
    };

/** A handle of a selected key, where it is drawn (`kx`, `ky`: its key). */
interface HandleHit {
  readonly curve: GraphCurve;
  readonly index: number;
  readonly side: HandleSide;
  readonly x: number;
  readonly y: number;
  readonly kx: number;
  readonly ky: number;
}

/** Pixels a press must travel before it is a drag. */
const DRAG_THRESHOLD = 3;
/** One wheel notch zooms by this. */
const WHEEL_ZOOM = 1.15;
/** The most samples a curve is drawn with. */
const MAX_SAMPLES = 4096;
/** Keys closer together than this many pixels on average are not drawn one by one. */
const KEY_COLLAPSE_PX = 3;
/** Handles are drawn for at most this many selected keys. */
const MAX_HANDLES = 64;

const bezierOf = (interp: CurveValue | undefined): BezierNumbers | null =>
  interp !== undefined && interp.kind === "bezier" ? interp : null;

const NO_POINTS: readonly GraphPointRef[] = [];

export class GraphView {
  readonly canvas: HTMLCanvasElement;
  events: GraphEvents = {};
  /** Frames drawn and the time the last one took (the frame-rate probe reads these). */
  readonly stats = { frames: 0, lastMs: 0, maxMs: 0 };

  #options: GraphOptions;
  #curves: readonly GraphCurve[] = [];
  #byId = new Map<string, GraphCurve>();
  #ranges = new Map<string, { lo: number; hi: number }>();
  #selection = new Map<string, Set<number>>();
  #viewport: GraphViewport | null = null;
  #playhead: number | null = null;
  #normalized = false;
  #tangentMode: TangentMode = "free";
  #theme: GraphTheme | null = null;
  #fixedTheme = false;
  #width = 0;
  #height = 0;
  #dpr = 1;
  #drag: Drag | null = null;
  #marquee: Box | null = null;
  #guide: number | null = null;
  #samples = new Float64Array(0);
  #frames: GraphFrames | null;
  #frame = 0;
  #dirty = false;
  #teardown: (() => void)[] = [];
  #viewAttr = "";
  #destroyed = false;

  constructor(
    canvas: HTMLCanvasElement,
    options: GraphOptions,
    extra: { readonly theme?: GraphTheme; readonly frames?: GraphFrames } = {},
  ) {
    this.canvas = canvas;
    this.#options = options;
    if (extra.theme !== undefined) {
      this.#theme = extra.theme;
      this.#fixedTheme = true;
    }
    this.#frames = extra.frames ?? null;
  }

  // ---- data ---------------------------------------------------------------------------------

  setOptions(options: GraphOptions): void {
    this.#options = options;
    this.invalidate();
  }

  setCurves(curves: readonly GraphCurve[]): void {
    if (curves === this.#curves) return;
    this.#curves = curves;
    this.#byId.clear();
    this.#ranges.clear();
    for (const c of curves) {
      this.#byId.set(c.id, c);
      this.#ranges.set(c.id, valueRange(c.values));
    }
    this.invalidate();
  }

  get curves(): readonly GraphCurve[] {
    return this.#curves;
  }

  setSelection(points: readonly GraphPointRef[]): void {
    this.#selection.clear();
    for (const p of points) {
      let set = this.#selection.get(p.curve);
      if (set === undefined) {
        set = new Set();
        this.#selection.set(p.curve, set);
      }
      set.add(p.index);
    }
    this.invalidate();
  }

  setPlayhead(t: number | null): void {
    if (t === this.#playhead) return;
    this.#playhead = t;
    this.invalidate();
  }

  setNormalized(on: boolean): void {
    if (on === this.#normalized) return;
    this.#normalized = on;
    this.invalidate();
  }

  setTangentMode(mode: TangentMode): void {
    this.#tangentMode = mode;
  }

  /** The viewport to show; `null` fits the curves when they are first drawn. */
  setViewport(viewport: GraphViewport | null): void {
    this.#viewport = viewport;
    this.invalidate();
  }

  get viewport(): GraphViewport | null {
    return this.#viewport;
  }

  /** Whether a drag of keys or of a handle is in progress. */
  get dragging(): boolean {
    return this.#drag !== null && (this.#drag.kind === "move" || this.#drag.kind === "tangent");
  }

  // ---- geometry -----------------------------------------------------------------------------

  /** Where the curves are drawn, in CSS pixels of the canvas. */
  plot(): PlotRect {
    const th = this.#theme;
    const left = th?.rulerWidth ?? 0;
    const top = th?.rulerHeight ?? 0;
    return {
      left,
      top,
      width: Math.max(1, this.#width - left),
      height: Math.max(1, this.#height - top),
    };
  }

  /** A curve's value as the viewport's value axis has it. */
  #shown(curve: GraphCurve, v: number): number {
    return this.#normalized ? normalize(v, this.#rangeOf(curve.id)) : v;
  }

  #rangeOf(id: string): { lo: number; hi: number } {
    const drag = this.#drag;
    if (drag !== null && drag.kind === "move") {
      const frozen = drag.ranges.get(id);
      if (frozen !== undefined) return frozen;
    }
    if (drag !== null && drag.kind === "tangent" && drag.curve === id) return drag.range;
    return this.#ranges.get(id) ?? { lo: 0, hi: 1 };
  }

  /** The viewport that shows every key (`selected`: the selected ones, when there are any). */
  fitted(selected = false): GraphViewport | null {
    let tMin = Number.POSITIVE_INFINITY;
    let tMax = Number.NEGATIVE_INFINITY;
    let vMin = Number.POSITIVE_INFINITY;
    let vMax = Number.NEGATIVE_INFINITY;
    const take = (curve: GraphCurve, i: number): void => {
      const t = curve.times[i] as number;
      const v = this.#shown(curve, curve.values[i] as number);
      if (t < tMin) tMin = t;
      if (t > tMax) tMax = t;
      if (v < vMin) vMin = v;
      if (v > vMax) vMax = v;
    };
    const some = selected && this.#selection.size > 0;
    for (const curve of this.#curves) {
      if (some) {
        for (const i of this.#selection.get(curve.id) ?? []) {
          if (i < curve.times.length) take(curve, i);
        }
      } else {
        for (let i = 0; i < curve.times.length; i++) take(curve, i);
      }
    }
    if (!(tMax >= tMin)) return null;
    const extent: Extent = { tMin, tMax, vMin, vMax };
    return fitExtent(extent, this.#options.second, 1);
  }

  /** Fits the curves (or the selected keys) and tells the host. */
  fit(selected = false): void {
    const vp = this.fitted(selected);
    if (vp === null) return;
    this.#viewport = vp;
    this.events.viewport?.(vp);
    this.invalidate();
  }

  /** Zooms both axes about the middle of the plot. */
  zoomBy(factor: number, axis: "time" | "value" | "both" = "time"): void {
    const vp = this.#viewport;
    if (vp === null) return;
    const plot = this.plot();
    this.#setViewportByHand(
      zoomAbout(
        vp,
        plot,
        plot.left + plot.width / 2,
        plot.top + plot.height / 2,
        axis === "value" ? 1 : factor,
        axis === "time" ? 1 : factor,
        this.#limits(),
      ),
    );
  }

  #limits() {
    const o = this.#options;
    return {
      minTime: o.frame * 2,
      maxTime: o.second * 3600 * 24,
      minValue: 1e-6,
      maxValue: 1e9,
    };
  }

  #setViewportByHand(vp: GraphViewport): void {
    this.#viewport = vp;
    this.events.viewport?.(vp);
    this.invalidate();
  }

  // ---- canvas, theme, frames ----------------------------------------------------------------

  /** Sizes the canvas (CSS pixels) at a device pixel ratio and draws at once. */
  resize(cssWidth: number, cssHeight: number, dpr = 1): void {
    const w = Math.max(1, Math.floor(cssWidth));
    const h = Math.max(1, Math.floor(cssHeight));
    const bw = Math.max(1, Math.round(w * dpr));
    const bh = Math.max(1, Math.round(h * dpr));
    const backing = this.canvas.width !== bw || this.canvas.height !== bh;
    if (!backing && this.#width === w && this.#height === h && this.#dpr === dpr) return;
    this.#dpr = dpr;
    if (backing) {
      this.canvas.width = bw;
      this.canvas.height = bh;
    }
    this.#width = w;
    this.#height = h;
    // A resize clears the canvas between layout and paint: draw now, not on the next frame.
    this.#dirty = true;
    this.flush();
  }

  refreshTheme(): void {
    if (this.#fixedTheme) return;
    try {
      this.#theme = readGraphTheme(this.canvas.parentElement ?? this.canvas);
    } catch {
      // The window's stylesheets are still loading: keep what we have and read again later.
    }
    this.invalidate();
  }

  invalidate(): void {
    if (this.#dirty || this.#destroyed) return;
    this.#dirty = true;
    const frames = this.#frames ?? this.#windowFrames();
    if (frames === null) return;
    this.#frame = frames.request(() => {
      this.#frame = 0;
      this.flush();
    });
  }

  #windowFrames(): GraphFrames | null {
    const win = this.canvas.ownerDocument?.defaultView;
    if (win === null || win === undefined || typeof win.requestAnimationFrame !== "function") {
      return null;
    }
    return {
      request: (cb) => win.requestAnimationFrame(cb),
      cancel: (h) => win.cancelAnimationFrame(h),
    };
  }

  /** Draws now if anything changed. */
  flush(): void {
    if (!this.#dirty || this.#destroyed) return;
    this.#dirty = false;
    if (this.#theme === null || this.#width === 0) return;
    if (this.#viewport === null) {
      const vp = this.fitted();
      if (vp !== null) this.#viewport = vp;
    }
    const ctx = this.canvas.getContext("2d");
    if (ctx === null) return;
    const started = performance.now();
    this.#draw(ctx, this.#theme);
    const took = performance.now() - started;
    this.stats.frames += 1;
    this.stats.lastMs = took;
    if (took > this.stats.maxMs) this.stats.maxMs = took;
    this.#writeViewAttribute();
  }

  /** The viewport and the plot as an attribute: what assistive tools and the flows read. */
  #writeViewAttribute(): void {
    const vp = this.#viewport;
    const plot = this.plot();
    const text =
      vp === null
        ? ""
        : `${vp.t0} ${vp.t1} ${vp.v0} ${vp.v1} ${plot.left} ${plot.top} ${plot.width} ${plot.height}`;
    if (text === this.#viewAttr) return;
    this.#viewAttr = text;
    this.canvas.setAttribute("data-viewport", text);
  }

  /** Attaches the input listeners, the size and theme observers. Call `destroy()` to undo. */
  bind(): void {
    const c = this.canvas;
    const doc = c.ownerDocument;
    const win = doc.defaultView;
    const on = <K extends keyof HTMLElementEventMap>(
      type: K,
      fn: (e: HTMLElementEventMap[K]) => void,
      options?: AddEventListenerOptions,
    ): void => {
      c.addEventListener(type, fn, options);
      this.#teardown.push(() => c.removeEventListener(type, fn, options));
    };
    on("pointerdown", (e) => this.#pointerDown(e));
    on("pointermove", (e) => this.#pointerMove(e));
    on("pointerup", (e) => this.#pointerUp(e));
    on("pointercancel", () => this.cancelDrag());
    on("dblclick", (e) => this.#doubleClick(e));
    on("wheel", (e) => this.#wheel(e), { passive: false });
    on("keydown", (e) => this.#keyDown(e));

    this.refreshTheme();
    const parent = c.parentElement;
    const Observer = win?.ResizeObserver ?? globalThis.ResizeObserver;
    if (parent !== null && Observer !== undefined) {
      const ro = new Observer((entries) => {
        const box = entries[entries.length - 1]?.contentRect;
        if (box) this.resize(box.width, box.height, win?.devicePixelRatio ?? 1);
      });
      ro.observe(parent);
      this.#teardown.push(() => ro.disconnect());
      const r = parent.getBoundingClientRect();
      if (r.width > 0 && r.height > 0) this.resize(r.width, r.height, win?.devicePixelRatio ?? 1);
    }
    const Mutations = win?.MutationObserver ?? globalThis.MutationObserver;
    if (!this.#fixedTheme && Mutations !== undefined) {
      const mo = new Mutations(() => this.refreshTheme());
      mo.observe(doc.documentElement, {
        attributes: true,
        attributeFilter: ["data-theme", "data-density", "class", "style"],
      });
      this.#teardown.push(() => mo.disconnect());
      // A detached window copies its stylesheets as links: read the tokens again once loaded.
      const loaded = (): void => this.refreshTheme();
      doc.addEventListener("load", loaded, true);
      this.#teardown.push(() => doc.removeEventListener("load", loaded, true));
    }
    const fonts = doc.fonts;
    if (fonts !== undefined && typeof fonts.addEventListener === "function") {
      const fn = (): void => this.refreshTheme();
      fonts.addEventListener("loadingdone", fn);
      this.#teardown.push(() => fonts.removeEventListener("loadingdone", fn));
    }
    if (win !== null && typeof win.matchMedia === "function") {
      let mq: MediaQueryList | null = null;
      const onChange = (): void => {
        this.resize(this.#width, this.#height, win.devicePixelRatio);
        arm();
      };
      const arm = (): void => {
        mq?.removeEventListener("change", onChange);
        mq = win.matchMedia(`(resolution: ${win.devicePixelRatio}dppx)`);
        mq.addEventListener("change", onChange);
      };
      arm();
      this.#teardown.push(() => mq?.removeEventListener("change", onChange));
    }
  }

  /** Detaches what `bind()` attached (the canvas is moving to another window, or going away). */
  unbind(): void {
    this.cancelDrag();
    if (this.#frame !== 0) (this.#frames ?? this.#windowFrames())?.cancel(this.#frame);
    this.#frame = 0;
    this.#dirty = false;
    for (const t of this.#teardown) t();
    this.#teardown = [];
  }

  destroy(): void {
    this.unbind();
    this.#destroyed = true;
  }

  // ---- drawing ------------------------------------------------------------------------------

  #draw(ctx: CanvasRenderingContext2D, th: GraphTheme): void {
    const w = this.#width;
    const h = this.#height;
    const plot = this.plot();
    ctx.setTransform(this.#dpr, 0, 0, this.#dpr, 0, 0);
    ctx.fillStyle = th.surface;
    ctx.fillRect(0, 0, w, h);
    const vp = this.#viewport;
    if (vp === null) {
      this.#drawRulerBacks(ctx, th, plot, w, h);
      return;
    }
    this.#drawGrid(ctx, th, vp, plot, w, h);

    ctx.save();
    ctx.beginPath();
    ctx.rect(plot.left, plot.top, plot.width, plot.height);
    ctx.clip();
    const n = Math.min(MAX_SAMPLES, Math.max(2, Math.ceil(plot.width)));
    if (this.#samples.length !== n) this.#samples = new Float64Array(n);
    for (const curve of this.#curves) this.#drawCurve(ctx, th, vp, plot, curve, n);
    for (const curve of this.#curves) this.#drawKeys(ctx, th, vp, plot, curve);
    this.#drawHandles(ctx, th, vp, plot);
    if (this.#guide !== null) {
      const x = Math.round(xOf(vp, plot, this.#guide)) + 0.5;
      ctx.strokeStyle = th.guide;
      ctx.lineWidth = th.line;
      ctx.beginPath();
      ctx.moveTo(x, plot.top);
      ctx.lineTo(x, plot.top + plot.height);
      ctx.stroke();
    }
    const box = this.#marquee;
    if (box !== null) {
      ctx.fillStyle = th.marquee;
      ctx.fillRect(box.x0, box.y0, box.x1 - box.x0, box.y1 - box.y0);
      ctx.strokeStyle = th.selected;
      ctx.lineWidth = th.line;
      ctx.strokeRect(box.x0 + 0.5, box.y0 + 0.5, box.x1 - box.x0, box.y1 - box.y0);
    }
    ctx.restore();

    if (this.#playhead !== null) {
      const x = Math.round(xOf(vp, plot, this.#playhead)) + 0.5;
      if (x >= plot.left && x <= w) {
        ctx.strokeStyle = th.playhead;
        ctx.lineWidth = th.line;
        ctx.beginPath();
        ctx.moveTo(x, 0);
        ctx.lineTo(x, h);
        ctx.stroke();
      }
    }
  }

  #drawRulerBacks(
    ctx: CanvasRenderingContext2D,
    th: GraphTheme,
    plot: PlotRect,
    w: number,
    h: number,
  ): void {
    ctx.fillStyle = th.ruler;
    ctx.fillRect(0, 0, w, plot.top);
    ctx.fillRect(0, 0, plot.left, h);
  }

  #drawGrid(
    ctx: CanvasRenderingContext2D,
    th: GraphTheme,
    vp: GraphViewport,
    plot: PlotRect,
    w: number,
    h: number,
  ): void {
    const o = this.#options;
    this.#drawRulerBacks(ctx, th, plot, w, h);
    ctx.font = th.font;
    ctx.lineWidth = th.line;
    ctx.textBaseline = "middle";

    // Time: a label needs about eight characters of room.
    const labelPx = th.fontPx * 7;
    const unitsPerPx = (vp.t1 - vp.t0) / plot.width;
    const stepT = timeStep(labelPx * unitsPerPx, o.second, o.frame);
    ctx.textAlign = "left";
    for (const t of ticks(vp.t0, vp.t1, stepT)) {
      const x = Math.round(xOf(vp, plot, t)) + 0.5;
      ctx.strokeStyle = th.grid;
      ctx.beginPath();
      ctx.moveTo(x, plot.top - th.pad);
      ctx.lineTo(x, h);
      ctx.stroke();
      ctx.fillStyle = th.text;
      ctx.fillText(o.formatTime(t), x + th.pad, plot.top / 2);
    }

    // Value: a label every three text lines at the closest.
    const stepV = niceStep(vp.v1 - vp.v0, Math.max(1, Math.floor(plot.height / (th.fontPx * 3))));
    ctx.textAlign = "right";
    for (const v of ticks(vp.v0, vp.v1, stepV)) {
      const y = Math.round(yOf(vp, plot, v)) + 0.5;
      if (y < plot.top + th.fontPx / 2) continue;
      ctx.strokeStyle = v === 0 ? th.gridStrong : th.grid;
      ctx.beginPath();
      ctx.moveTo(plot.left - th.pad, y);
      ctx.lineTo(w, y);
      ctx.stroke();
      ctx.fillStyle = th.text;
      const shown = this.#normalized ? `${Math.round(v * 100)}%` : o.formatValue(v);
      ctx.fillText(shown, plot.left - th.pad * 2, y);
    }
  }

  #drawCurve(
    ctx: CanvasRenderingContext2D,
    th: GraphTheme,
    vp: GraphViewport,
    plot: PlotRect,
    curve: GraphCurve,
    n: number,
  ): void {
    if (curve.times.length === 0) return;
    const out = this.#samples;
    curve.sample(vp.t0, vp.t1, n, out);
    const range = this.#rangeOf(curve.id);
    const normalized = this.#normalized;
    const scaleY = plot.height / (vp.v1 - vp.v0);
    const stepX = plot.width / (n - 1);
    ctx.strokeStyle = th.curves[curve.color % th.curves.length] as string;
    ctx.lineWidth = th.lineStrong;
    ctx.globalAlpha = curve.locked === true ? th.dimmed : 1;
    ctx.beginPath();
    for (let i = 0; i < n; i++) {
      const raw = out[i] as number;
      const v = normalized ? normalize(raw, range) : raw;
      const y = plot.top + (vp.v1 - v) * scaleY;
      const x = plot.left + i * stepX;
      if (i === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    ctx.stroke();
    ctx.globalAlpha = 1;
  }

  #drawKeys(
    ctx: CanvasRenderingContext2D,
    th: GraphTheme,
    vp: GraphViewport,
    plot: PlotRect,
    curve: GraphCurve,
  ): void {
    const times = curve.times;
    const first = lowerBound(times, vp.t0);
    const last = lowerBound(times, vp.t1 + 1);
    const count = last - first;
    if (count <= 0) return;
    const selected = this.#selection.get(curve.id);
    const half = th.keySize / 2;
    const color = th.curves[curve.color % th.curves.length] as string;
    // Too dense to tell keys apart: only the selected ones are marked.
    const dense = count * KEY_COLLAPSE_PX > plot.width;
    for (let i = first; i < last; i++) {
      const isSelected = selected?.has(i) === true;
      if (dense && !isSelected) continue;
      const x = xOf(vp, plot, times[i] as number);
      const y = yOf(vp, plot, this.#shown(curve, curve.values[i] as number));
      ctx.beginPath();
      ctx.moveTo(x, y - half);
      ctx.lineTo(x + half, y);
      ctx.lineTo(x, y + half);
      ctx.lineTo(x - half, y);
      ctx.closePath();
      ctx.fillStyle = isSelected ? th.selected : th.surface;
      ctx.fill();
      ctx.strokeStyle = isSelected ? th.onSelected : color;
      ctx.lineWidth = th.line;
      ctx.stroke();
    }
  }

  /** The handles of the selected keys that are on screen, in screen positions. */
  #handles(vp: GraphViewport, plot: PlotRect): HandleHit[] {
    const out: HandleHit[] = [];
    let budget = MAX_HANDLES;
    for (const curve of this.#curves) {
      const selected = this.#selection.get(curve.id);
      if (selected === undefined || curve.locked === true) continue;
      for (const i of selected) {
        if (budget <= 0) return out;
        if (i >= curve.times.length) continue;
        budget -= 1;
        const kx = xOf(vp, plot, curve.times[i] as number);
        const ky = yOf(vp, plot, this.#shown(curve, curve.values[i] as number));
        const push = (side: HandleSide, seg: Segment, b: BezierNumbers): void => {
          const p = handlePoint(seg, b, side);
          out.push({
            curve,
            index: i,
            side,
            x: xOf(vp, plot, p.t),
            y: yOf(vp, plot, this.#shown(curve, p.v)),
            kx,
            ky,
          });
        };
        const outB = bezierOf(curve.interps[i]);
        if (outB !== null && i + 1 < curve.times.length) push("out", segmentOf(curve, i), outB);
        const inB = i > 0 ? bezierOf(curve.interps[i - 1]) : null;
        if (inB !== null) push("in", segmentOf(curve, i - 1), inB);
      }
    }
    return out;
  }

  #drawHandles(
    ctx: CanvasRenderingContext2D,
    th: GraphTheme,
    vp: GraphViewport,
    plot: PlotRect,
  ): void {
    const handles = this.#handles(vp, plot);
    if (handles.length === 0) return;
    ctx.strokeStyle = th.selected;
    ctx.lineWidth = th.line;
    ctx.beginPath();
    for (const hnd of handles) {
      ctx.moveTo(hnd.kx, hnd.ky);
      ctx.lineTo(hnd.x, hnd.y);
    }
    ctx.stroke();
    for (const hnd of handles) {
      ctx.beginPath();
      ctx.arc(hnd.x, hnd.y, th.handleSize / 2, 0, Math.PI * 2);
      ctx.fillStyle = th.surface;
      ctx.fill();
      ctx.stroke();
    }
  }

  // ---- hit testing --------------------------------------------------------------------------

  #local(e: { clientX: number; clientY: number }): { x: number; y: number } {
    const r = this.canvas.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }

  #hitKey(x: number, y: number): GraphPointRef | null {
    const vp = this.#viewport;
    const th = this.#theme;
    if (vp === null || th === null) return null;
    const plot = this.plot();
    let best: { ref: GraphPointRef; distance: number; selected: boolean } | null = null;
    for (const curve of this.#curves) {
      const hit = nearestKey(
        curve.times,
        vp,
        plot,
        (i) => yOf(vp, plot, this.#shown(curve, curve.values[i] as number)),
        x,
        y,
        th.hitRadius,
      );
      if (hit === null) continue;
      // A selected key wins over an unselected one at the same place (the other component of a
      // position, say), then the nearest.
      const selected = this.#selection.get(curve.id)?.has(hit.index) === true;
      if (
        best === null ||
        (selected && !best.selected) ||
        (selected === best.selected && hit.distance < best.distance)
      ) {
        best = { ref: { curve: curve.id, index: hit.index }, distance: hit.distance, selected };
      }
    }
    return best?.ref ?? null;
  }

  #hitHandle(x: number, y: number): HandleHit | null {
    const vp = this.#viewport;
    const th = this.#theme;
    if (vp === null || th === null) return null;
    let best: HandleHit | null = null;
    let bestD = th.hitRadius;
    for (const hnd of this.#handles(vp, this.plot())) {
      const d = Math.hypot(hnd.x - x, hnd.y - y);
      if (d <= bestD) {
        best = hnd;
        bestD = d;
      }
    }
    return best;
  }

  /** The curve whose line passes nearest under (`x`, `y`), within reach. */
  #hitCurve(x: number, y: number): GraphCurve | null {
    const vp = this.#viewport;
    const th = this.#theme;
    if (vp === null || th === null) return null;
    const plot = this.plot();
    const t = tAt(vp, plot, x);
    const one = new Float64Array(2);
    let best: GraphCurve | null = null;
    let bestD = th.hitRadius * 2;
    for (const curve of this.#curves) {
      if (curve.times.length === 0) continue;
      curve.sample(t, t + 1, 2, one);
      const d = Math.abs(yOf(vp, plot, this.#shown(curve, one[0] as number)) - y);
      if (d <= bestD) {
        best = curve;
        bestD = d;
      }
    }
    return best;
  }

  // ---- input --------------------------------------------------------------------------------

  #pointerDown(e: PointerEvent): void {
    const vp = this.#viewport;
    if (vp === null || this.#drag !== null) return;
    const { x, y } = this.#local(e);
    const plot = this.plot();
    this.canvas.focus({ preventScroll: true });
    const capture = (): void => {
      try {
        this.canvas.setPointerCapture(e.pointerId);
      } catch {
        // A synthetic pointer has nothing to capture.
      }
    };
    if (e.button === 1 || (e.button === 0 && e.altKey && !this.#hitHandle(x, y))) {
      e.preventDefault();
      this.#drag = { kind: "pan", x, y };
      this.canvas.style.cursor = "grabbing";
      capture();
      return;
    }
    if (e.button !== 0) return;
    if (y < plot.top) {
      if (x >= plot.left) {
        this.#drag = { kind: "scrub" };
        capture();
        this.events.scrub?.(tAt(vp, plot, x));
      }
      return;
    }
    if (x < plot.left) return;
    const handle = this.#hitHandle(x, y);
    if (handle !== null) {
      const { curve, index } = handle;
      const hasNext = index + 1 < curve.times.length;
      this.#drag = {
        kind: "tangent",
        curve: curve.id,
        index,
        side: handle.side,
        next: hasNext ? segmentOf(curve, index) : null,
        prev: index > 0 ? segmentOf(curve, index - 1) : null,
        nextB: hasNext ? bezierOf(curve.interps[index]) : null,
        prevB: index > 0 ? bezierOf(curve.interps[index - 1]) : null,
        range: this.#rangeOf(curve.id),
        started: false,
        last: null,
      };
      capture();
      return;
    }
    const key = this.#hitKey(x, y);
    const additive = e.shiftKey || e.ctrlKey || e.metaKey;
    if (key !== null) {
      const wasSelected = this.#selection.get(key.curve)?.has(key.index) === true;
      if (!wasSelected) this.events.select?.([key], additive ? "add" : "replace");
      const curve = this.#byId.get(key.curve);
      if (curve === undefined || curve.locked === true) return;
      // The select event above may already have changed the selection (the host sets it).
      const curves = new Set<string>([key.curve]);
      for (const id of this.#selection.keys()) curves.add(id);
      const ranges = new Map<string, { lo: number; hi: number }>();
      for (const id of curves) ranges.set(id, this.#ranges.get(id) ?? { lo: 0, hi: 1 });
      this.#drag = {
        kind: "move",
        x0: x,
        y0: y,
        grabbed: key,
        origT: curve.times[key.index] as number,
        ranges,
        curves: [...curves],
        toggleOnClick: wasSelected && additive,
        started: false,
        last: null,
      };
      capture();
      return;
    }
    this.#drag = { kind: "marquee", x0: x, y0: y, x, y, add: additive };
    capture();
  }

  #pointerMove(e: PointerEvent): void {
    const drag = this.#drag;
    const vp = this.#viewport;
    if (vp === null) return;
    const { x, y } = this.#local(e);
    const plot = this.plot();
    if (drag === null) {
      const over =
        y >= plot.top && x >= plot.left && (this.#hitHandle(x, y) ?? this.#hitKey(x, y)) !== null;
      const cursor = over ? "pointer" : "";
      if (this.canvas.style.cursor !== cursor) this.canvas.style.cursor = cursor;
      return;
    }
    switch (drag.kind) {
      case "pan":
        this.#setViewportByHand(panBy(vp, plot, x - drag.x, y - drag.y));
        drag.x = x;
        drag.y = y;
        return;
      case "scrub":
        this.events.scrub?.(tAt(vp, plot, Math.max(plot.left, x)));
        return;
      case "marquee":
        drag.x = x;
        drag.y = y;
        if (Math.hypot(x - drag.x0, y - drag.y0) >= DRAG_THRESHOLD || this.#marquee !== null) {
          this.#marquee = boxOf(drag.x0, drag.y0, x, y);
          this.invalidate();
        }
        return;
      case "move": {
        if (!drag.started && Math.hypot(x - drag.x0, y - drag.y0) < DRAG_THRESHOLD) return;
        drag.started = true;
        let dx = x - drag.x0;
        let dy = y - drag.y0;
        if (e.shiftKey) ({ dx, dy } = constrainAxis(dx, dy));
        const unitsPerPx = (vp.t1 - vp.t0) / plot.width;
        let dt = 0;
        let guide: number | null = null;
        if (dx !== 0) {
          const raw = drag.origT + dx * unitsPerPx;
          const snap = e.ctrlKey || e.metaKey ? undefined : this.#options.snap;
          const snapped = snap?.(raw, unitsPerPx) ?? { t: raw, guide: null };
          dt = Math.round(snapped.t - drag.origT);
          guide = snapped.guide;
        }
        const perPx = (vp.v1 - vp.v0) / plot.height;
        const shownDv = quantize(-dy * perPx, perPx);
        const dv: Record<string, number> = {};
        if (shownDv !== 0) {
          for (const id of drag.curves) {
            const range = drag.ranges.get(id) ?? { lo: 0, hi: 1 };
            dv[id] = this.#normalized ? denormalizeDelta(shownDv, range) : shownDv;
          }
        }
        if (guide !== this.#guide) {
          this.#guide = guide;
          this.invalidate();
        }
        drag.last = { type: "move", grabbed: drag.grabbed, dt, dv };
        this.events.gesture?.("update", drag.last);
        return;
      }
      case "tangent": {
        const t = tAt(vp, plot, x);
        const shown = vAt(vp, plot, y);
        const v = this.#normalized
          ? drag.range.lo + shown * (drag.range.hi - drag.range.lo || 1)
          : shown;
        const mode: TangentMode = e.altKey ? "free" : this.#tangentMode;
        let edit: GraphEdit | null = null;
        if (drag.side === "out" && drag.next !== null && drag.nextB !== null) {
          const n = handleNumbers(drag.next, { t, v }, drag.nextB.y1);
          const opposite =
            drag.prev !== null && drag.prevB !== null
              ? oppositeOfOut(mode, drag.next, n.x, n.y, drag.prev, drag.prevB.x2, drag.prevB.y2)
              : null;
          edit = {
            type: "tangent",
            curve: drag.curve,
            index: drag.index,
            side: "out",
            ...n,
            opposite,
          };
        } else if (drag.side === "in" && drag.prev !== null && drag.prevB !== null) {
          const n = handleNumbers(drag.prev, { t, v }, drag.prevB.y2);
          const opposite =
            drag.next !== null && drag.nextB !== null
              ? oppositeOfIn(mode, drag.prev, n.x, n.y, drag.next, drag.nextB.x1, drag.nextB.y1)
              : null;
          edit = {
            type: "tangent",
            curve: drag.curve,
            index: drag.index,
            side: "in",
            ...n,
            opposite,
          };
        }
        if (edit === null) return;
        drag.started = true;
        drag.last = edit;
        this.events.gesture?.("update", edit);
        return;
      }
    }
  }

  #pointerUp(e: PointerEvent): void {
    const drag = this.#drag;
    if (drag === null) return;
    this.#drag = null;
    try {
      this.canvas.releasePointerCapture(e.pointerId);
    } catch {
      // Nothing was captured.
    }
    const vp = this.#viewport;
    switch (drag.kind) {
      case "pan":
        this.canvas.style.cursor = "";
        return;
      case "scrub":
        return;
      case "marquee": {
        const box = this.#marquee;
        this.#marquee = null;
        this.invalidate();
        if (box === null || vp === null) {
          if (!drag.add) this.events.select?.(NO_POINTS, "replace");
          return;
        }
        const plot = this.plot();
        const points: GraphPointRef[] = [];
        for (const curve of this.#curves) {
          const inside = keysInBox(
            curve.times,
            vp,
            plot,
            (i) => yOf(vp, plot, this.#shown(curve, curve.values[i] as number)),
            box,
          );
          for (const index of inside) points.push({ curve: curve.id, index });
        }
        this.events.select?.(points, drag.add ? "add" : "replace");
        return;
      }
      case "move":
        if (this.#guide !== null) {
          this.#guide = null;
          this.invalidate();
        }
        if (drag.started) this.events.gesture?.("commit", drag.last);
        else if (drag.toggleOnClick) this.events.select?.([drag.grabbed], "toggle");
        else if (!(e.shiftKey || e.ctrlKey || e.metaKey)) {
          // A plain click on one of several selected keys leaves only that key selected.
          this.events.select?.([drag.grabbed], "replace");
        }
        return;
      case "tangent":
        if (drag.started) this.events.gesture?.("commit", drag.last);
        return;
    }
  }

  /** Ends a drag in progress without its edit (Escape, a lost pointer, the panel closing). */
  cancelDrag(): void {
    const drag = this.#drag;
    if (drag === null) return;
    this.#drag = null;
    this.#marquee = null;
    this.#guide = null;
    this.canvas.style.cursor = "";
    if ((drag.kind === "move" || drag.kind === "tangent") && drag.started) {
      this.events.gesture?.("cancel", null);
    }
    this.invalidate();
  }

  #doubleClick(e: MouseEvent): void {
    const vp = this.#viewport;
    if (vp === null) return;
    const { x, y } = this.#local(e);
    const plot = this.plot();
    if (y < plot.top || x < plot.left) return;
    if (this.#hitKey(x, y) !== null) {
      this.events.action?.("easing");
      return;
    }
    const curve = this.#hitCurve(x, y) ?? (this.#curves.length === 1 ? this.#curves[0] : null);
    if (curve === null || curve === undefined || curve.locked === true) return;
    this.events.add?.(curve.id, tAt(vp, plot, x));
  }

  #wheel(e: WheelEvent): void {
    const vp = this.#viewport;
    if (vp === null) return;
    e.preventDefault();
    const { x, y } = this.#local(e);
    const plot = this.plot();
    const delta = e.deltaY !== 0 ? e.deltaY : e.deltaX;
    if (e.ctrlKey || e.metaKey || e.altKey) {
      const f = delta < 0 ? WHEEL_ZOOM : 1 / WHEEL_ZOOM;
      const both = e.altKey;
      const value = e.shiftKey && !both;
      this.#setViewportByHand(
        zoomAbout(vp, plot, x, y, value ? 1 : f, value || both ? f : 1, this.#limits()),
      );
      return;
    }
    if (e.shiftKey) this.#setViewportByHand(panBy(vp, plot, -delta, 0));
    else this.#setViewportByHand(panBy(vp, plot, -e.deltaX, -e.deltaY));
  }

  #keyDown(e: KeyboardEvent): void {
    const mod = e.ctrlKey || e.metaKey;
    const done = (): void => {
      e.preventDefault();
      e.stopPropagation();
    };
    const big = e.shiftKey ? 10 : 1;
    const key = e.key;
    if (key === "Escape") {
      if (this.#drag !== null) {
        this.cancelDrag();
        done();
      } else if (this.#selection.size > 0) {
        this.events.action?.("clear");
        done();
      }
      return;
    }
    if (e.altKey && !mod && (key === "ArrowLeft" || key === "ArrowRight")) {
      this.events.action?.(key === "ArrowLeft" ? "previousKey" : "nextKey");
      done();
      return;
    }
    if (e.altKey) return;
    if (mod) {
      const lower = key.toLowerCase();
      if (lower === "a") this.events.action?.("selectAll");
      else if (lower === "c") this.events.action?.("copy");
      else if (lower === "v") this.events.action?.("paste");
      else return;
      done();
      return;
    }
    switch (key) {
      case "ArrowLeft":
        this.events.nudge?.(-big, 0);
        break;
      case "ArrowRight":
        this.events.nudge?.(big, 0);
        break;
      case "ArrowUp":
        this.events.nudge?.(0, big);
        break;
      case "ArrowDown":
        this.events.nudge?.(0, -big);
        break;
      case "Delete":
      case "Backspace":
        this.events.action?.("delete");
        break;
      case "Enter":
        this.events.action?.("easing");
        break;
      case "f":
      case "F":
        this.fit(e.shiftKey);
        break;
      case "+":
      case "=":
        this.zoomBy(1.25, "both");
        break;
      case "-":
      case "_":
        this.zoomBy(0.8, "both");
        break;
      default:
        return;
    }
    done();
  }

  /** The key under a point of the window (a right-click opens its menu), or null. */
  keyAtClient(clientX: number, clientY: number): GraphPointRef | null {
    const at = this.#local({ clientX, clientY });
    const plot = this.plot();
    if (at.x < plot.left || at.y < plot.top) return null;
    return this.#hitKey(at.x, at.y);
  }

  /** The first selected key and where it is in the window (the menu key opens its menu there). */
  selectedAtClient(): { ref: GraphPointRef; clientX: number; clientY: number } | null {
    const vp = this.#viewport;
    if (vp === null) return null;
    const plot = this.plot();
    const r = this.canvas.getBoundingClientRect();
    for (const curve of this.#curves) {
      const set = this.#selection.get(curve.id);
      if (set === undefined) continue;
      for (const index of set) {
        if (index >= curve.times.length) continue;
        return {
          ref: { curve: curve.id, index },
          clientX: r.left + xOf(vp, plot, curve.times[index] as number),
          clientY: r.top + yOf(vp, plot, this.#shown(curve, curve.values[index] as number)),
        };
      }
    }
    return null;
  }
}

/** The segment from key `i` of the curve to the next key. */
export function segmentOf(curve: GraphCurve, i: number): Segment {
  return {
    t0: curve.times[i] as number,
    v0: curve.values[i] as number,
    t1: curve.times[i + 1] as number,
    v1: curve.values[i + 1] as number,
  };
}

/** A value change rounded to a tenth of what one pixel is worth: no noise digits. */
export function quantize(dv: number, perPx: number): number {
  if (dv === 0 || !(perPx > 0)) return 0;
  const q = 10 ** (Math.floor(Math.log10(perPx)) - 1);
  return Number((Math.round(dv / q) * q).toPrecision(12));
}
