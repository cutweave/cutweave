// SPDX-License-Identifier: GPL-3.0-or-later
// What a handle drag means (WP-2H.06): pure functions from the placement a gesture started with
// and the pointer (canvas pixels) to the new property values. The values go out as
// `item.set_props`; nothing here touches the document.
//
// Scale keeps the opposite handle where it is on the canvas; rotation turns about the anchor;
// moving the anchor keeps the picture still (the position moves with it); a crop edge is dragged
// in item space and written to the model's edge that shows there (a horizontally flipped item
// shows `crop.right` on its visible left edge; same for top and bottom with a vertical flip).
import {
  applyAffine,
  type CropEdges,
  type LayerGeom,
  type LayerState,
  layerGeometry,
  type Pt,
  type Size,
} from "./geometry.ts";

export type ScaleHandle = "tl" | "t" | "tr" | "r" | "br" | "b" | "bl" | "l";
/** An edge of the visible rectangle in item space. */
export type ItemEdge = "left" | "top" | "right" | "bottom";

export const MIN_SCALE = 0.01;
/** The least a crop leaves visible, as a fraction of the picture. */
export const MIN_VISIBLE = 0.02;
/** Shift snaps rotation to multiples of this many degrees. */
export const ROTATE_STEP = 15;

/** The handle's place on the visible rectangle, in item pixels. */
export function handleItemPoint(visible: LayerGeom["visible"], handle: ScaleHandle): Pt {
  const [x0, y0, x1, y1] = visible;
  const xm = (x0 + x1) / 2;
  const ym = (y0 + y1) / 2;
  switch (handle) {
    case "tl":
      return { x: x0, y: y0 };
    case "t":
      return { x: xm, y: y0 };
    case "tr":
      return { x: x1, y: y0 };
    case "r":
      return { x: x1, y: ym };
    case "br":
      return { x: x1, y: y1 };
    case "b":
      return { x: xm, y: y1 };
    case "bl":
      return { x: x0, y: y1 };
    case "l":
      return { x: x0, y: ym };
  }
}

const OPPOSITE: Readonly<Record<ScaleHandle, ScaleHandle>> = {
  tl: "br",
  t: "b",
  tr: "bl",
  r: "l",
  br: "tl",
  b: "t",
  bl: "tr",
  l: "r",
};

/** The handle's place on the canvas. */
export const handleCanvasPoint = (geom: LayerGeom, handle: ScaleHandle): Pt =>
  applyAffine(geom.toCanvas, handleItemPoint(geom.visible, handle));

/** The position (canvas fractions) that puts the anchor at canvas point `at`. */
const positionOf = (at: Pt, canvas: Size): Pt => ({
  x: at.x / canvas.w - 0.5,
  y: at.y / canvas.h - 0.5,
});

/** Rotation by `deg` clockwise of canvas vector `v`. */
function turn(v: Pt, deg: number): Pt {
  const r = (deg * Math.PI) / 180;
  const [s, c] = [Math.sin(r), Math.cos(r)];
  return { x: c * v.x - s * v.y, y: s * v.x + c * v.y };
}

/** Moves the clip by a canvas-pixel delta. */
export function moveBy(start: LayerState, delta: Pt, canvas: Size): Pt {
  return { x: start.position.x + delta.x / canvas.w, y: start.position.y + delta.y / canvas.h };
}

export interface ScaleResult {
  readonly scale: Pt;
  readonly position: Pt;
}

/**
 * Scales from a handle, the opposite handle fixed. A corner scales both axes by the same factor
 * unless `free`; an edge handle scales its own axis only.
 */
export function scaleDrag(
  start: LayerState,
  geom: LayerGeom,
  canvas: Size,
  handle: ScaleHandle,
  pointer: Pt,
  free: boolean,
): ScaleResult {
  const opposite = handleItemPoint(geom.visible, OPPOSITE[handle]);
  const held = applyAffine(geom.toCanvas, opposite);
  const grabbed = handleItemPoint(geom.visible, handle);
  // The pointer relative to the fixed handle, in the canvas axes turned back by the rotation.
  const u = turn({ x: pointer.x - held.x, y: pointer.y - held.y }, -start.rotation);
  const dx = (grabbed.x - opposite.x) * geom.fit * start.scale.x;
  const dy = (grabbed.y - opposite.y) * geom.fit * start.scale.y;
  let fx = 1;
  let fy = 1;
  if (dx !== 0 && dy !== 0) {
    if (free) {
      fx = u.x / dx;
      fy = u.y / dy;
    } else {
      fx = fy = (u.x * dx + u.y * dy) / (dx * dx + dy * dy);
    }
  } else if (dx !== 0) {
    fx = u.x / dx;
  } else if (dy !== 0) {
    fy = u.y / dy;
  }
  const sx = Math.max(MIN_SCALE, start.scale.x * (Number.isFinite(fx) ? fx : 1));
  const sy = Math.max(MIN_SCALE, start.scale.y * (Number.isFinite(fy) ? fy : 1));
  // Put the fixed handle back where it was.
  const ax = start.anchor.x * geom.display.w;
  const ay = start.anchor.y * geom.display.h;
  const offset = turn(
    { x: (opposite.x - ax) * geom.fit * sx, y: (opposite.y - ay) * geom.fit * sy },
    start.rotation,
  );
  return {
    scale: { x: sx, y: sy },
    position: positionOf({ x: held.x - offset.x, y: held.y - offset.y }, canvas),
  };
}

const wrap = (deg: number): number => {
  const d = ((((deg + 180) % 360) + 360) % 360) - 180;
  return d === -180 ? 180 : d;
};

/** Rotates about the anchor by the angle the pointer swept since `from`; `snap` rounds to steps. */
export function rotateDrag(
  start: LayerState,
  geom: LayerGeom,
  from: Pt,
  pointer: Pt,
  snap: boolean,
): number {
  const angle = (p: Pt) => (Math.atan2(p.y - geom.anchorAt.y, p.x - geom.anchorAt.x) * 180) / Math.PI;
  const turned = start.rotation + wrap(angle(pointer) - angle(from));
  return snap ? Math.round(turned / ROTATE_STEP) * ROTATE_STEP : turned;
}

export interface AnchorResult {
  readonly anchor: Pt;
  readonly position: Pt;
}

/** Moves the anchor to the pointer without moving the picture. */
export function anchorDrag(geom: LayerGeom, canvas: Size, pointer: Pt): AnchorResult {
  const item = applyAffine(geom.fromCanvas, pointer);
  return {
    anchor: { x: item.x / geom.display.w, y: item.y / geom.display.h },
    position: positionOf(pointer, canvas),
  };
}

/** The visible rectangle's removed fractions in item space: left, top, right, bottom. */
function itemEdges(geom: LayerGeom): CropEdges {
  const [x0, y0, x1, y1] = geom.visible;
  return {
    left: x0 / geom.display.w,
    top: y0 / geom.display.h,
    right: 1 - x1 / geom.display.w,
    bottom: 1 - y1 / geom.display.h,
  };
}

/** Item-space removed fractions to the model's four edges (the flip swaps the mirrored pair). */
export function toModelCrop(edges: CropEdges, flipH: boolean, flipV: boolean): CropEdges {
  return {
    left: flipH ? edges.right : edges.left,
    right: flipH ? edges.left : edges.right,
    top: flipV ? edges.bottom : edges.top,
    bottom: flipV ? edges.top : edges.bottom,
  };
}

const clamp = (v: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, v));

/**
 * Drags one or two item-space edges to the pointer. `mirror` (Shift) crops the opposite edge by
 * the same amount. Returns the model's four crop edges.
 */
export function cropDrag(
  start: LayerState,
  geom: LayerGeom,
  edges: readonly ItemEdge[],
  pointer: Pt,
  mirror: boolean,
): CropEdges {
  const p = applyAffine(geom.fromCanvas, pointer);
  const e = { ...itemEdges(geom) };
  const room = 1 - MIN_VISIBLE;
  for (const edge of edges) {
    if (edge === "left" || edge === "right") {
      const other = edge === "left" ? "right" : "left";
      const want = edge === "left" ? p.x / geom.display.w : 1 - p.x / geom.display.w;
      const most = mirror ? room / 2 : room - e[other];
      const value = clamp(want, 0, Math.max(0, most));
      e[edge] = value;
      if (mirror) e[other] = value;
    } else {
      const other = edge === "top" ? "bottom" : "top";
      const want = edge === "top" ? p.y / geom.display.h : 1 - p.y / geom.display.h;
      const most = mirror ? room / 2 : room - e[other];
      const value = clamp(want, 0, Math.max(0, most));
      e[edge] = value;
      if (mirror) e[other] = value;
    }
  }
  return toModelCrop(e, start.flipH, start.flipV);
}

export type PlaceMode = "fit" | "fill" | "center";

/**
 * Fit (the visible picture inside the canvas), Fill (covering it) or Center, for the visible
 * (cropped) rectangle. Scale becomes uniform for Fit and Fill; rotation, flip and anchor stay.
 */
export function placeOnCanvas(
  start: LayerState,
  geom: LayerGeom,
  canvas: Size,
  mode: PlaceMode,
): ScaleResult {
  const [x0, y0, x1, y1] = geom.visible;
  const vw = x1 - x0;
  const vh = y1 - y0;
  let scale = start.scale;
  if (mode !== "center") {
    const rx = canvas.w / (geom.fit * vw);
    const ry = canvas.h / (geom.fit * vh);
    const s = Math.max(MIN_SCALE, mode === "fit" ? Math.min(rx, ry) : Math.max(rx, ry));
    scale = { x: s, y: s };
  }
  const ax = start.anchor.x * geom.display.w;
  const ay = start.anchor.y * geom.display.h;
  const offset = turn(
    {
      x: ((x0 + x1) / 2 - ax) * geom.fit * scale.x,
      y: ((y0 + y1) / 2 - ay) * geom.fit * scale.y,
    },
    start.rotation,
  );
  return {
    scale,
    position: positionOf({ x: canvas.w / 2 - offset.x, y: canvas.h / 2 - offset.y }, canvas),
  };
}

/** The geometry after applying a partial placement, for re-reading handles mid-drag. */
export function withPlacement(
  start: LayerState,
  patch: Partial<LayerState>,
  display: Size,
  canvas: Size,
): LayerGeom | null {
  return layerGeometry({ ...start, ...patch }, display, canvas);
}
