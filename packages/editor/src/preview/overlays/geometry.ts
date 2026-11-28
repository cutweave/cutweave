// SPDX-License-Identifier: GPL-3.0-or-later
// Where a clip's picture sits on the canvas, for the preview's handles (WP-2H.06, arch §8).
//
// This is the TypeScript twin of `cutweave-render`'s `LayerGeometry` (geometry.rs, WP-2C.04): the
// render core is not shared with the editor as wasm yet, so the maths is ported (`geometry.test.ts`
// pins the same cases) and a change there changes here.
//
// Canvas pixels are the sequence's own (`format.width x height`); item pixels are the displayed
// picture's pixels (coded size turned by the display rotation), mirrored on the flipped axes. The
// order is: crop mask, flip, then scale, rotation about the anchor and position. `position` is
// the anchor's offset from the canvas centre in canvas fractions, `scale` multiplies the
// aspect-fit size, `anchor` is in picture fractions. Crop masks without moving the picture: the
// anchor and the fitted size refer to the whole picture.
import type { Animatable, ClipItem as Item, MediaAsset } from "@cutweave/protocol";
import { displaySize } from "../overlay.ts";

export interface Pt {
  readonly x: number;
  readonly y: number;
}

/** `[a, b, c, d, e, f]`: `x' = a x + b y + c`, `y' = d x + e y + f`. */
export type Affine = readonly [number, number, number, number, number, number];

export interface Size {
  readonly w: number;
  readonly h: number;
}

export interface CropEdges {
  readonly left: number;
  readonly top: number;
  readonly right: number;
  readonly bottom: number;
}

/** A clip's placement at one moment: the values the engine evaluates at the playhead. */
export interface LayerState {
  readonly position: Pt;
  readonly scale: Pt;
  readonly rotation: number;
  readonly anchor: Pt;
  readonly crop: CropEdges;
  readonly flipH: boolean;
  readonly flipV: boolean;
}

export const DEFAULT_LAYER: LayerState = {
  position: { x: 0, y: 0 },
  scale: { x: 1, y: 1 },
  rotation: 0,
  anchor: { x: 0.5, y: 0.5 },
  crop: { left: 0, top: 0, right: 0, bottom: 0 },
  flipH: false,
  flipV: false,
};

export const applyAffine = (m: Affine, p: Pt): Pt => ({
  x: m[0] * p.x + m[1] * p.y + m[2],
  y: m[3] * p.x + m[4] * p.y + m[5],
});

export function invertAffine(m: Affine): Affine | null {
  const [a, b, c, d, e, f] = m;
  const det = a * e - b * d;
  const norm = Math.max(Math.abs(a), Math.abs(b), Math.abs(d), Math.abs(e));
  if (!Number.isFinite(det) || norm === 0 || Math.abs(det) <= 1e-12 * norm * norm) return null;
  const ia = e / det;
  const ib = -b / det;
  const id = -d / det;
  const ie = a / det;
  const inv: Affine = [ia, ib, -(ia * c + ib * f), id, ie, -(id * c + ie * f)];
  return inv.every(Number.isFinite) ? inv : null;
}

/** The picture's rectangle and where it lands. All canvas values are in canvas pixels. */
export interface LayerGeom {
  /** The displayed picture's size in item pixels. */
  readonly display: Size;
  /** Aspect-fit factor: item pixels to canvas pixels at scale 1. */
  readonly fit: number;
  readonly toCanvas: Affine;
  readonly fromCanvas: Affine;
  /** The visible (cropped, flipped) rectangle in item pixels: `[x0, y0, x1, y1]`. */
  readonly visible: readonly [number, number, number, number];
  /** Its corners on the canvas: top-left, top-right, bottom-right, bottom-left (item order). */
  readonly corners: readonly [Pt, Pt, Pt, Pt];
  /** The axis-aligned bounds of those corners: `[x0, y0, x1, y1]`. */
  readonly bounds: readonly [number, number, number, number];
  /** Where the anchor lands on the canvas. */
  readonly anchorAt: Pt;
}

const unit = (v: number): number => (Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : 0);

/** The crop rectangle in item pixels, mirrored by the flip (render's `visible_rect`). */
export function visibleRect(
  state: LayerState,
  display: Size,
): readonly [number, number, number, number] {
  let left = unit(state.crop.left);
  let right = unit(state.crop.right);
  let top = unit(state.crop.top);
  let bottom = unit(state.crop.bottom);
  if (state.flipH) [left, right] = [right, left];
  if (state.flipV) [top, bottom] = [bottom, top];
  return [left * display.w, top * display.h, (1 - right) * display.w, (1 - bottom) * display.h];
}

/** `null` when the layer cannot be drawn (degenerate scale, nothing visible, empty canvas). */
export function layerGeometry(state: LayerState, display: Size, canvas: Size): LayerGeom | null {
  if (canvas.w <= 0 || canvas.h <= 0 || display.w <= 0 || display.h <= 0) return null;
  const fit = Math.min(canvas.w / display.w, canvas.h / display.h);
  const kx = fit * state.scale.x;
  const ky = fit * state.scale.y;
  const rad = (state.rotation * Math.PI) / 180;
  const s = Math.sin(rad);
  const c = Math.cos(rad);
  const [m00, m01, m10, m11] = [c * kx, -s * ky, s * kx, c * ky];
  const ax = state.anchor.x * display.w;
  const ay = state.anchor.y * display.h;
  const px = canvas.w * (0.5 + state.position.x);
  const py = canvas.h * (0.5 + state.position.y);
  const toCanvas: Affine = [m00, m01, px - m00 * ax - m01 * ay, m10, m11, py - m10 * ax - m11 * ay];
  if (!toCanvas.every(Number.isFinite)) return null;
  const fromCanvas = invertAffine(toCanvas);
  if (fromCanvas === null) return null;
  const visible = visibleRect(state, display);
  if (visible[2] <= visible[0] || visible[3] <= visible[1]) return null;
  const corners = [
    { x: visible[0], y: visible[1] },
    { x: visible[2], y: visible[1] },
    { x: visible[2], y: visible[3] },
    { x: visible[0], y: visible[3] },
  ].map((p) => applyAffine(toCanvas, p)) as unknown as [Pt, Pt, Pt, Pt];
  const xs = corners.map((p) => p.x);
  const ys = corners.map((p) => p.y);
  return {
    display,
    fit,
    toCanvas,
    fromCanvas,
    visible,
    corners,
    bounds: [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)],
    anchorAt: { x: px, y: py },
  };
}

// ---- Evaluating animated properties ---------------------------------------------------------

type Lerp<T> = (a: T, b: T, p: number) => T;

const lerpNumber: Lerp<number> = (a, b, p) => a + (b - a) * p;
const lerpVec: Lerp<Pt> = (a, b, p) => ({ x: lerpNumber(a.x, b.x, p), y: lerpNumber(a.y, b.y, p) });
const hold = <T>(_a: T, b: T): T => b;

/** The y of a CSS cubic Bezier (0,0)-(x1,y1)-(x2,y2)-(1,1) at time `x`. */
function bezier(x1: number, y1: number, x2: number, y2: number, x: number): number {
  const at = (a: number, b: number, u: number) =>
    3 * a * (1 - u) * (1 - u) * u + 3 * b * (1 - u) * u * u + u * u * u;
  let lo = 0;
  let hi = 1;
  for (let i = 0; i < 40; i++) {
    const mid = (lo + hi) / 2;
    if (at(x1, x2, mid) < x) lo = mid;
    else hi = mid;
  }
  return at(y1, y2, (lo + hi) / 2);
}

interface Key<T> {
  readonly t: number;
  readonly value: T;
  readonly interp: { kind: string; x1?: number; y1?: number; x2?: number; y2?: number };
}

/**
 * The value of a property at item time `t` (flicks from the item's start): the static value, or
 * the keyframes' (hold until the next key; linear and Bezier eased; a spring is taken as linear,
 * which only picks the value a drag starts from).
 */
export function valueAt<T>(prop: Animatable<T>, t: number, lerp: Lerp<T>): T {
  if (typeof prop !== "object" || prop === null || !("keyframes" in prop)) return prop as T;
  const keys = (prop as unknown as { keyframes: Key<T>[] }).keyframes;
  const first = keys[0];
  if (first === undefined) return prop as T;
  if (t <= first.t) return first.value;
  for (let i = 1; i < keys.length; i++) {
    const next = keys[i];
    const prev = keys[i - 1];
    if (next === undefined || prev === undefined || t >= next.t) continue;
    const span = next.t - prev.t;
    const u = span > 0 ? (t - prev.t) / span : 1;
    if (prev.interp.kind === "hold") return prev.value;
    const i2 = prev.interp;
    const p = i2.kind === "bezier" ? bezier(i2.x1 ?? 0, i2.y1 ?? 0, i2.x2 ?? 1, i2.y2 ?? 1, u) : u;
    return lerp(prev.value, next.value, p);
  }
  return keys[keys.length - 1]?.value ?? first.value;
}

const num = (v: unknown, fallback: number): number =>
  typeof v === "number" && Number.isFinite(v) ? v : fallback;
const vec = (v: unknown, fallback: Pt): Pt => {
  const o = (typeof v === "object" && v !== null ? v : {}) as Record<string, unknown>;
  return { x: num(o.x, fallback.x), y: num(o.y, fallback.y) };
};

/** A clip's placement at sequence time `playhead` (flicks), as the engine evaluates it. */
export function layerAt(item: Item, playhead: number): LayerState {
  const visual = (item as { visual?: Record<string, Record<string, unknown> | undefined> }).visual;
  const t = playhead - num(item.start, 0);
  const transform = visual?.transform;
  const crop = visual?.crop;
  const flip = visual?.flip;
  const read = <T>(prop: unknown, lerp: Lerp<T>): T | undefined =>
    prop === undefined ? undefined : valueAt(prop as Animatable<T>, t, lerp);
  const edge = (name: keyof CropEdges): number => num(read(crop?.[name], lerpNumber), 0);
  return {
    position: vec(read(transform?.position, lerpVec), DEFAULT_LAYER.position),
    scale: vec(read(transform?.scale, lerpVec), DEFAULT_LAYER.scale),
    rotation: num(read(transform?.rotation, lerpNumber), 0),
    anchor: vec(read(transform?.anchor, lerpVec), DEFAULT_LAYER.anchor),
    crop: { left: edge("left"), top: edge("top"), right: edge("right"), bottom: edge("bottom") },
    flipH: read(flip?.horizontal, hold<boolean>) === true,
    flipV: read(flip?.vertical, hold<boolean>) === true,
  };
}

/** The displayed picture's size of a clip's media; the canvas when the probe has none (stub). */
export function pictureSize(media: MediaAsset | undefined, canvas: Size): Size {
  const d = displaySize(media);
  return d === null ? canvas : { w: d[0], h: d[1] };
}

// ---- Coordinates: viewport CSS px <-> canvas (sequence) px <-> normalized --------------------

/** The overlay's box on screen and the canvas it shows. */
export interface CanvasView {
  /** Viewport CSS px of the box's top-left corner, and its size. */
  readonly left: number;
  readonly top: number;
  readonly width: number;
  readonly height: number;
  readonly canvas: Size;
}

/** CSS pixels per canvas pixel (the preview zoom). */
export const viewScale = (v: CanvasView): number => (v.width > 0 ? v.width / v.canvas.w : 1);

/** A viewport point (clientX/Y) to canvas pixels. */
export function clientToCanvas(v: CanvasView, clientX: number, clientY: number): Pt {
  return {
    x: ((clientX - v.left) / v.width) * v.canvas.w,
    y: ((clientY - v.top) / v.height) * v.canvas.h,
  };
}

/** Canvas pixels to a point in the overlay's own box (CSS px from its top-left). */
export function canvasToLocal(v: CanvasView, p: Pt): Pt {
  return { x: (p.x / v.canvas.w) * v.width, y: (p.y / v.canvas.h) * v.height };
}

/** Canvas pixels to fractions of the canvas, `0..1` across it. */
export const canvasToNormalized = (canvas: Size, p: Pt): Pt => ({
  x: p.x / canvas.w,
  y: p.y / canvas.h,
});

export const normalizedToCanvas = (canvas: Size, p: Pt): Pt => ({
  x: p.x * canvas.w,
  y: p.y * canvas.h,
});
