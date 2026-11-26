// SPDX-License-Identifier: GPL-3.0-or-later
// Handle maths (WP-2H.06): the new values a drag produces, checked against the picture's own
// geometry (what lands where on the canvas), at several canvases, rotations and flips.
import { describe, expect, it } from "vitest";
import {
  applyAffine,
  type CanvasView,
  canvasToLocal,
  canvasToNormalized,
  clientToCanvas,
  DEFAULT_LAYER,
  type LayerState,
  layerAt,
  layerGeometry,
  type Size,
  valueAt,
  viewScale,
} from "./geometry.ts";
import { canvasTargets, guideSegment, snapBounds, snapPoint, targetsWith } from "./snapping.ts";
import {
  anchorDrag,
  cropDrag,
  handleCanvasPoint,
  moveBy,
  placeOnCanvas,
  rotateDrag,
  scaleDrag,
  toModelCrop,
} from "./transformOps.ts";

const canvas: Size = { w: 1920, h: 1080 };
const pic: Size = { w: 1920, h: 1080 };
const state = (over: Partial<LayerState> = {}): LayerState => ({ ...DEFAULT_LAYER, ...over });
const geomOf = (s: LayerState, display: Size = pic, c: Size = canvas) => {
  const g = layerGeometry(s, display, c);
  if (g === null) throw new Error("no geometry");
  return g;
};
const near = (a: number, b: number, eps = 1e-6) => expect(Math.abs(a - b)).toBeLessThan(eps);

describe("layer geometry", () => {
  it("fits a 16:9 picture to a 16:9 canvas, and letterboxes it on 9:16", () => {
    expect(geomOf(state()).bounds).toEqual([0, 0, 1920, 1080]);
    const portrait = geomOf(state(), pic, { w: 1080, h: 1920 });
    near(portrait.bounds[0], 0);
    near(portrait.bounds[2], 1080);
    near(portrait.bounds[1], (1920 - 607.5) / 2);
  });

  it("places the anchor at position, and turns about it", () => {
    const s = state({ position: { x: 0.25, y: -0.1 }, rotation: 90, anchor: { x: 0, y: 0 } });
    const g = geomOf(s);
    near(g.anchorAt.x, 1920 * 0.75);
    near(g.anchorAt.y, 1080 * 0.4);
    const corner = applyAffine(g.toCanvas, { x: 0, y: 0 });
    near(corner.x, g.anchorAt.x);
    near(corner.y, g.anchorAt.y);
    // Clockwise: the picture's top-right corner ends up below the anchor.
    const tr = applyAffine(g.toCanvas, { x: 1920, y: 0 });
    near(tr.x, g.anchorAt.x);
    expect(tr.y).toBeGreaterThan(g.anchorAt.y);
  });

  it("crop masks without moving the picture", () => {
    const g = geomOf(state({ crop: { left: 0.25, top: 0, right: 0.25, bottom: 0 } }));
    expect(g.bounds).toEqual([480, 0, 1440, 1080]);
    const whole = applyAffine(g.toCanvas, { x: 0, y: 0 });
    expect(whole).toEqual({ x: 0, y: 0 });
  });

  it("a flipped item shows crop.right on its visible left edge", () => {
    const g = geomOf(state({ flipH: true, crop: { left: 0, top: 0, right: 0.25, bottom: 0 } }));
    expect(g.visible[0]).toBe(480);
    expect(g.visible[2]).toBe(1920);
    expect(toModelCrop({ left: 0.25, top: 0, right: 0, bottom: 0 }, true, false)).toEqual({
      left: 0,
      top: 0,
      right: 0.25,
      bottom: 0,
    });
  });

  it("is null for nothing visible or a zero scale", () => {
    expect(
      layerGeometry(state({ crop: { left: 0.5, top: 0, right: 0.5, bottom: 0 } }), pic, canvas),
    ).toBe(null);
    expect(layerGeometry(state({ scale: { x: 0, y: 1 } }), pic, canvas)).toBe(null);
  });
});

describe("evaluating animated values", () => {
  const keys = (a: number, b: number, interp: { kind: string } = { kind: "linear" }): never =>
    ({
      keyframes: [
        { t: 0, value: a, interp },
        { t: 100, value: b, interp },
      ],
    }) as never;
  const lerp = (a: number, b: number, p: number) => a + (b - a) * p;
  it("interpolates, holds and clamps", () => {
    expect(valueAt(keys(0, 10), 50, lerp)).toBe(5);
    expect(valueAt(keys(0, 10, { kind: "hold" }), 99, lerp)).toBe(0);
    expect(valueAt(keys(0, 10), -5, lerp)).toBe(0);
    expect(valueAt(keys(0, 10), 500, lerp)).toBe(10);
    expect(valueAt(7, 3, lerp)).toBe(7);
  });
  it("reads a clip at the playhead, item-relative", () => {
    const item = {
      start: 50,
      visual: { transform: { rotation: keys(0, 90), position: { x: 0.1, y: 0 } } },
    } as never;
    expect(layerAt(item, 100).rotation).toBe(45);
    expect(layerAt(item, 100).position.x).toBe(0.1);
    expect(layerAt(item, 100).scale).toEqual({ x: 1, y: 1 });
  });
});

describe("coordinates", () => {
  const view: CanvasView = { left: 100, top: 50, width: 480, height: 270, canvas };
  it("maps client px to canvas px and back, at any zoom", () => {
    expect(viewScale(view)).toBe(0.25);
    expect(clientToCanvas(view, 100 + 240, 50 + 135)).toEqual({ x: 960, y: 540 });
    expect(canvasToLocal(view, { x: 960, y: 540 })).toEqual({ x: 240, y: 135 });
    expect(canvasToNormalized(canvas, { x: 480, y: 270 })).toEqual({ x: 0.25, y: 0.25 });
  });
});

describe("move", () => {
  it("shifts the position by canvas px as a fraction of the canvas", () => {
    const p = moveBy(state({ position: { x: 0.1, y: 0 } }), { x: 96, y: -54 }, canvas);
    near(p.x, 0.15);
    near(p.y, -0.05);
  });
});

describe("scale", () => {
  const cases: [string, Size, number][] = [
    ["16:9", { w: 1920, h: 1080 }, 0],
    ["9:16", { w: 1080, h: 1920 }, 0],
    ["rotated", { w: 1920, h: 1080 }, 30],
  ];
  for (const [name, c, rotation] of cases) {
    it(`keeps the opposite corner fixed (${name})`, () => {
      const s = state({ rotation, position: { x: 0.05, y: 0.02 } });
      const g = geomOf(s, pic, c);
      const fixed = handleCanvasPoint(g, "tl");
      const grab = handleCanvasPoint(g, "br");
      // Drag the bottom-right corner outwards along the diagonal by 50 %.
      const pointer = {
        x: fixed.x + (grab.x - fixed.x) * 1.5,
        y: fixed.y + (grab.y - fixed.y) * 1.5,
      };
      const out = scaleDrag(s, g, c, "br", pointer, false);
      near(out.scale.x, 1.5);
      near(out.scale.y, 1.5);
      const after = geomOf({ ...s, ...out }, pic, c);
      const stillThere = handleCanvasPoint(after, "tl");
      near(stillThere.x, fixed.x, 1e-6);
      near(stillThere.y, fixed.y, 1e-6);
      const moved = handleCanvasPoint(after, "br");
      near(moved.x, pointer.x, 1e-6);
      near(moved.y, pointer.y, 1e-6);
    });
  }

  it("a corner is uniform, Shift frees it; an edge scales one axis", () => {
    const g = geomOf(state());
    const pointer = { x: 1920 * 1.5, y: 1080 };
    const uniform = scaleDrag(state(), g, canvas, "br", pointer, false);
    near(uniform.scale.x, uniform.scale.y);
    const free = scaleDrag(state(), g, canvas, "br", pointer, true);
    near(free.scale.x, 1.5);
    near(free.scale.y, 1);
    const edge = scaleDrag(state(), g, canvas, "r", { x: 960 + 960 * 1.2, y: 300 }, false);
    near(edge.scale.x, 1.1);
    near(edge.scale.y, 1);
  });

  it("never reaches zero or flips", () => {
    const g = geomOf(state());
    const out = scaleDrag(state(), g, canvas, "br", { x: -500, y: -500 }, false);
    expect(out.scale.x).toBeGreaterThan(0);
    expect(out.scale.y).toBeGreaterThan(0);
  });
});

describe("rotate", () => {
  it("turns by the angle the pointer sweeps about the anchor, 15 degrees to 0.5", () => {
    const g = geomOf(state());
    const from = { x: g.anchorAt.x + 400, y: g.anchorAt.y };
    const r = (15 * Math.PI) / 180;
    const to = { x: g.anchorAt.x + 400 * Math.cos(r), y: g.anchorAt.y + 400 * Math.sin(r) };
    near(rotateDrag(state(), g, from, to, false), 15, 1e-9);
    near(
      rotateDrag(state({ rotation: 10 }), geomOf(state({ rotation: 10 })), from, to, false),
      25,
      1e-9,
    );
    // Shift rounds to 15-degree steps.
    const slightly = {
      x: g.anchorAt.x + 400 * Math.cos(0.3),
      y: g.anchorAt.y + 400 * Math.sin(0.3),
    };
    expect(rotateDrag(state(), g, from, slightly, true)).toBe(15);
  });
});

describe("anchor", () => {
  it("moves the anchor without moving the picture", () => {
    const s = state({ rotation: 20, position: { x: 0.1, y: 0.1 } });
    const g = geomOf(s);
    const pointer = { x: 700, y: 400 };
    const out = anchorDrag(g, canvas, pointer);
    const after = geomOf({ ...s, ...out });
    for (let i = 0; i < 4; i++) {
      near(after.corners[i]?.x ?? 0, g.corners[i]?.x ?? 1, 1e-6);
      near(after.corners[i]?.y ?? 0, g.corners[i]?.y ?? 1, 1e-6);
    }
  });
});

describe("crop", () => {
  it("drags the left edge to the pointer", () => {
    const g = geomOf(state());
    const c = cropDrag(state(), g, ["left"], { x: 480, y: 500 }, false);
    expect(c).toEqual({ left: 0.25, top: 0, right: 0, bottom: 0 });
  });
  it("Shift crops the opposite edge by the same amount", () => {
    const g = geomOf(state());
    const c = cropDrag(state(), g, ["right"], { x: 1440, y: 0 }, true);
    expect(c).toEqual({ left: 0.25, top: 0, right: 0.25, bottom: 0 });
  });
  it("maps through a flip: the visible left edge of a flipped item is crop.right", () => {
    const s = state({ flipH: true });
    const c = cropDrag(s, geomOf(s), ["left"], { x: 480, y: 0 }, false);
    expect(c.right).toBe(0.25);
    expect(c.left).toBe(0);
  });
  it("a corner crops two edges and never leaves nothing", () => {
    const g = geomOf(state());
    const c = cropDrag(state(), g, ["left", "top"], { x: 960, y: 540 }, false);
    expect(c.left).toBe(0.5);
    expect(c.top).toBe(0.5);
    const all = cropDrag(
      state({ crop: { left: 0, top: 0, right: 0.4, bottom: 0 } }),
      geomOf(state({ crop: { left: 0, top: 0, right: 0.4, bottom: 0 } })),
      ["left"],
      { x: 1900, y: 0 },
      false,
    );
    expect(1 - all.left - all.right).toBeGreaterThan(0.01);
  });
});

describe("fit, fill, center", () => {
  it("fills a 16:9 canvas with a 9:16 picture, and fits it inside", () => {
    const portrait: Size = { w: 1080, h: 1920 };
    const s = state();
    const g = geomOf(s, portrait);
    const fill = placeOnCanvas(s, g, canvas, "fill");
    const filled = geomOf({ ...s, ...fill }, portrait);
    near(filled.bounds[0], 0, 1e-6);
    near(filled.bounds[2], 1920, 1e-6);
    const fit = placeOnCanvas(s, g, canvas, "fit");
    const fitted = geomOf({ ...s, ...fit }, portrait);
    near(fitted.bounds[1], 0, 1e-6);
    near(fitted.bounds[3], 1080, 1e-6);
  });
  it("centers the visible part of a cropped, offset clip", () => {
    const s = state({
      position: { x: 0.3, y: 0.2 },
      crop: { left: 0.5, top: 0, right: 0, bottom: 0 },
    });
    const out = placeOnCanvas(s, geomOf(s), canvas, "center");
    const g = geomOf({ ...s, ...out });
    near((g.bounds[0] + g.bounds[2]) / 2, 960, 1e-6);
    near((g.bounds[1] + g.bounds[3]) / 2, 540, 1e-6);
    expect(out.scale).toEqual({ x: 1, y: 1 });
  });
});

describe("snap guide extent", () => {
  it("stops at the canvas edges", () => {
    const canvas = { w: 1920, h: 1080 };
    expect(guideSegment({ axis: "x", at: 960 }, canvas)).toEqual({
      x1: 960,
      x2: 960,
      y1: 0,
      y2: 1080,
    });
    expect(guideSegment({ axis: "y", at: 540 }, canvas)).toEqual({
      x1: 0,
      x2: 1920,
      y1: 540,
      y2: 540,
    });
  });
});

describe("snapping", () => {
  const targets = canvasTargets(canvas);
  it("catches the canvas edges, centre, halves and quarters", () => {
    const hit = snapBounds([5, 100, 965, 300], targets, 8);
    expect(hit.dx).toBe(-5);
    expect(hit.guides).toContainEqual({ axis: "x", at: 0 });
    // The box's right edge is within 8 of the centre line too; the nearer one wins.
    expect(snapBounds([100, 0, 955, 200], targets, 8).dx).toBe(5);
    expect(
      snapBounds([400, 0, 500, 200], targets, 2).guides.filter((g) => g.axis === "x"),
    ).toHaveLength(0);
  });
  it("catches another clip's edge, so two clips butt up", () => {
    const withNeighbour = targetsWith(canvas, [[0, 0, 700, 1080]]);
    expect(snapBounds([704, 0, 1400, 1080], withNeighbour, 8).dx).toBe(-4);
  });
  it("pulls a pointer onto a target per axis", () => {
    const out = snapPoint({ x: 962, y: 100 }, targets, 6);
    expect(out.point).toEqual({ x: 960, y: 100 });
  });
});

describe("two 16:9 clips side by side on a 16:9 canvas (WP-2H.06 task)", () => {
  // Per clip: Shift-drag the left crop edge a quarter in (crops both sides), then drag the clip
  // to its half, where the edge snaps to the canvas edge or the centre line. Each clip ends up
  // filling its half within 1 px, in two drags.
  const half = (side: "left" | "right") => {
    let s = state();
    const cropped = cropDrag(s, geomOf(s), ["left"], { x: 480, y: 540 }, true);
    s = { ...s, crop: cropped };
    expect(cropped).toEqual({ left: 0.25, top: 0, right: 0.25, bottom: 0 });
    const g = geomOf(s);
    // The visible part is 480..1440; the pointer stops 10 px short of the half it belongs to.
    const want = side === "left" ? -480 : 480;
    const delta = { x: want + (side === "left" ? 10 : -10), y: 0 };
    const snap = snapBounds(
      [g.bounds[0] + delta.x, g.bounds[1], g.bounds[2] + delta.x, g.bounds[3]],
      canvasTargets(canvas),
      12,
    );
    const position = moveBy(s, { x: delta.x + snap.dx, y: 0 }, canvas);
    return geomOf({ ...s, position }).bounds;
  };
  it("fills the left and the right half exactly", () => {
    const left = half("left");
    near(left[0], 0, 1e-6);
    near(left[2], 960, 1e-6);
    const right = half("right");
    near(right[0], 960, 1e-6);
    near(right[2], 1920, 1e-6);
    near(left[3] - left[1], 1080, 1e-6);
  });
});
