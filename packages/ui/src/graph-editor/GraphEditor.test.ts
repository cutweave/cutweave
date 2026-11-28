// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-2C.07: the graph editor's canvas. Curves are drawn from `sample` (the engine's math in the
// editor), once per change and never per key as an element; keys are clicked, box-selected and
// dragged in time and value (Shift keeps to one axis, snapping moves the time, Escape cancels);
// handles turn into bezier numbers with the tangent mode's opposite handle; the wheel zooms
// about the pointer; the keyboard nudges, deletes, steps and fits; axe finds nothing.
import { cleanup, fireEvent, render } from "@testing-library/react";
import axe from "axe-core";
import { createElement as h } from "react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import {
  countingContext,
  denseCurves,
  FIXTURE_OPTIONS,
  FRAME,
  fixtureTheme,
  positionCurves,
} from "./fixtures.ts";
import { GraphEditor, type GraphEditorHandle } from "./GraphEditor.tsx";
import { handlePoint, tAt, xOf, yOf } from "./geometry.ts";
import {
  type GraphCurve,
  type GraphEdit,
  type GraphEvents,
  type GraphOptions,
  type GraphPointRef,
  GraphView,
  quantize,
  segmentOf,
} from "./view.ts";

afterEach(cleanup);

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
});

const WIDTH = 864; // a plot of 800 by 400 beside the rulers
const HEIGHT = 424;

interface Rig {
  readonly view: GraphView;
  readonly canvas: HTMLCanvasElement;
  readonly calls: Record<string, number>;
  readonly events: Required<{ [K in keyof GraphEvents]: ReturnType<typeof vi.fn> }>;
  /** Runs the frames asked for; returns how many there were. */
  tick(): number;
  /** Page position of a key. */
  at(curve: string, index: number): { clientX: number; clientY: number };
}

function rig(curves: readonly GraphCurve[], options: GraphOptions = FIXTURE_OPTIONS): Rig {
  const canvas = document.createElement("canvas");
  document.body.appendChild(canvas);
  const { ctx, calls } = countingContext();
  canvas.getContext = (() => ctx) as unknown as HTMLCanvasElement["getContext"];
  const queue: (() => void)[] = [];
  const view = new GraphView(canvas, options, {
    theme: fixtureTheme(),
    frames: {
      request: (cb) => queue.push(cb),
      cancel: () => {
        queue.length = 0;
      },
    },
  });
  const events = {
    select: vi.fn(),
    gesture: vi.fn(),
    add: vi.fn(),
    nudge: vi.fn(),
    action: vi.fn(),
    scrub: vi.fn(),
    viewport: vi.fn(),
  };
  // The host sets the selection it is told about, as the editor does.
  events.select.mockImplementation((points: readonly GraphPointRef[], mode: string) => {
    if (mode === "replace") view.setSelection(points);
  });
  view.events = events as unknown as GraphEvents;
  view.bind();
  view.setCurves(curves);
  view.resize(WIDTH, HEIGHT, 1);
  const tick = (): number => {
    const run = queue.splice(0);
    for (const cb of run) cb();
    return run.length;
  };
  tick();
  const at = (id: string, index: number) => {
    const curve = curves.find((c) => c.id === id) as GraphCurve;
    const vp = view.viewport;
    if (vp === null) throw new Error("no viewport");
    return {
      clientX: xOf(vp, view.plot(), curve.times[index] as number),
      clientY: yOf(vp, view.plot(), curve.values[index] as number),
    };
  };
  return { view, canvas, calls, events, tick, at };
}

const down = (el: Element, p: object): boolean =>
  fireEvent.pointerDown(el, { button: 0, pointerId: 1, ...p });
const move = (el: Element, p: object): boolean => fireEvent.pointerMove(el, { pointerId: 1, ...p });
const up = (el: Element, p: object): boolean =>
  fireEvent.pointerUp(el, { button: 0, pointerId: 1, ...p });

const lastEdit = (r: Rig): GraphEdit => {
  const calls = r.events.gesture.mock.calls;
  return calls[calls.length - 1]?.[1] as GraphEdit;
};

describe("drawing", () => {
  it("fits the keys and draws each curve from its samples, one path per curve", () => {
    const curves = positionCurves();
    const sampled = curves.map((c) => vi.spyOn(c, "sample"));
    const r = rig(curves);
    const plot = r.view.plot();
    expect(plot).toEqual({ left: 64, top: 24, width: 800, height: 400 });
    // Every key is inside the fitted viewport.
    const vp = r.view.viewport;
    expect(vp?.t0).toBeLessThan(1000);
    expect(vp?.t1).toBeGreaterThan(5000);
    expect(vp?.v0).toBeLessThan(-400);
    expect(vp?.v1).toBeGreaterThan(300);
    for (const s of sampled) {
      const last = s.mock.calls[s.mock.calls.length - 1];
      expect(last?.slice(0, 3)).toEqual([vp?.t0, vp?.t1, 800]);
    }
    expect(r.canvas.getAttribute("data-viewport")).toBe(
      `${vp?.t0} ${vp?.t1} ${vp?.v0} ${vp?.v1} 64 24 800 400`,
    );
  });

  it("draws only when something changed, at most once per frame", () => {
    const r = rig(positionCurves());
    const frames = r.view.stats.frames;
    expect(r.tick()).toBe(0);
    r.view.setPlayhead(2000);
    r.view.setPlayhead(2500);
    r.view.setSelection([{ curve: "position.x", index: 1 }]);
    expect(r.tick()).toBe(1);
    expect(r.view.stats.frames - frames).toBe(1);
    r.view.setPlayhead(2500); // the same value: nothing to draw
    expect(r.tick()).toBe(0);
  });

  it("draws nothing per key when keys are too dense to tell apart", () => {
    const r = rig(denseCurves(2, 2000));
    // 2 x 2000 keys in 800 px: no diamonds (a closed path each), only the two curves.
    expect(r.calls.closePath ?? 0).toBe(0);
    const sparse = rig(positionCurves());
    expect(sparse.calls.closePath).toBe(6);
  });

  it("draws ten curves of five hundred keys in well under a frame", () => {
    const r = rig(denseCurves(10, 500));
    const took: number[] = [];
    const vp = r.view.viewport;
    if (vp === null) throw new Error("no viewport");
    for (let i = 0; i < 40; i++) {
      // A pan and a zoom step, as a drag or the wheel would make.
      r.view.setViewport({ ...vp, t0: vp.t0 + i * 37, t1: vp.t1 + i * 41 });
      r.tick();
      took.push(r.view.stats.lastMs);
    }
    took.sort((a, b) => a - b);
    // 55 frames per second leave 18 ms; the canvas work (sampling, paths) must be a fraction.
    expect(took[Math.floor(took.length / 2)] as number).toBeLessThan(9);
  });
});

describe("selecting", () => {
  it("selects the key under a click, adds with Shift and clears on the empty graph", () => {
    const r = rig(positionCurves());
    const a = r.at("position.x", 1);
    down(r.canvas, a);
    up(r.canvas, a);
    expect(r.events.select).toHaveBeenCalledWith([{ curve: "position.x", index: 1 }], "replace");
    r.events.select.mockClear();
    const b = r.at("position.y", 2);
    down(r.canvas, { ...b, shiftKey: true });
    up(r.canvas, { ...b, shiftKey: true });
    expect(r.events.select).toHaveBeenCalledWith([{ curve: "position.y", index: 2 }], "add");
    r.events.select.mockClear();
    down(r.canvas, { clientX: 100, clientY: 60 });
    up(r.canvas, { clientX: 100, clientY: 60 });
    expect(r.events.select).toHaveBeenCalledWith([], "replace");
    expect(r.events.gesture).not.toHaveBeenCalled();
  });

  it("takes a selected key out with Ctrl+click", () => {
    const r = rig(positionCurves());
    r.view.setSelection([{ curve: "position.x", index: 1 }]);
    const a = r.at("position.x", 1);
    down(r.canvas, { ...a, ctrlKey: true });
    up(r.canvas, { ...a, ctrlKey: true });
    expect(r.events.select).toHaveBeenCalledWith([{ curve: "position.x", index: 1 }], "toggle");
  });

  it("box-selects the keys of every curve inside a dragged box", () => {
    const r = rig(positionCurves());
    const a = r.at("position.x", 1); // (3000, 200)
    const b = r.at("position.y", 1); // (3000, -150)
    down(r.canvas, { clientX: a.clientX - 30, clientY: a.clientY - 30 });
    move(r.canvas, { clientX: b.clientX + 30, clientY: b.clientY + 30 });
    r.tick();
    expect(r.calls.strokeRect).toBe(1); // the box is drawn
    up(r.canvas, { clientX: b.clientX + 30, clientY: b.clientY + 30 });
    expect(r.events.select).toHaveBeenCalledWith(
      [
        { curve: "position.x", index: 1 },
        { curve: "position.y", index: 1 },
      ],
      "replace",
    );
  });
});

describe("dragging keys", () => {
  it("moves the selected keys in time and value: updates, then one commit", () => {
    const r = rig(positionCurves());
    const vp = r.view.viewport;
    if (vp === null) throw new Error("no viewport");
    const plot = r.view.plot();
    const a = r.at("position.x", 1);
    down(r.canvas, a);
    move(r.canvas, { clientX: a.clientX + 1, clientY: a.clientY }); // under the threshold
    expect(r.events.gesture).not.toHaveBeenCalled();
    move(r.canvas, { clientX: a.clientX + 80, clientY: a.clientY - 40 });
    const perPx = (vp.v1 - vp.v0) / plot.height;
    const want = {
      type: "move",
      grabbed: { curve: "position.x", index: 1 },
      dt: Math.round((80 / plot.width) * (vp.t1 - vp.t0)),
      dv: { "position.x": quantize(40 * perPx, perPx) },
    };
    expect(r.events.gesture).toHaveBeenLastCalledWith("update", want);
    up(r.canvas, { clientX: a.clientX + 80, clientY: a.clientY - 40 });
    expect(r.events.gesture).toHaveBeenLastCalledWith("commit", want);
    expect(r.events.gesture.mock.calls.filter((c) => c[0] === "commit")).toHaveLength(1);
  });

  it("keeps to one axis with Shift", () => {
    const r = rig(positionCurves());
    const a = r.at("position.x", 1);
    down(r.canvas, a);
    move(r.canvas, { clientX: a.clientX + 80, clientY: a.clientY - 20, shiftKey: true });
    expect(lastEdit(r)).toMatchObject({ dv: {} });
    expect((lastEdit(r) as { dt: number }).dt).toBeGreaterThan(0);
    move(r.canvas, { clientX: a.clientX + 10, clientY: a.clientY - 90, shiftKey: true });
    expect(lastEdit(r)).toMatchObject({ dt: 0 });
    expect((lastEdit(r) as { dv: Record<string, number> }).dv["position.x"]).toBeGreaterThan(0);
  });

  it("snaps the time through the host, draws the guide, and Ctrl turns it off", () => {
    const snap = vi.fn((t: number) => ({ t: Math.round(t / FRAME) * FRAME, guide: 3200 }));
    const r = rig(positionCurves(), { ...FIXTURE_OPTIONS, snap });
    const a = r.at("position.x", 1);
    down(r.canvas, a);
    move(r.canvas, { clientX: a.clientX + 33, clientY: a.clientY });
    expect(snap).toHaveBeenCalled();
    expect((lastEdit(r) as { dt: number }).dt % FRAME).toBe(0);
    snap.mockClear();
    move(r.canvas, { clientX: a.clientX + 34, clientY: a.clientY, ctrlKey: true });
    expect(snap).not.toHaveBeenCalled();
  });

  it("cancels on Escape: the host rolls back", () => {
    const r = rig(positionCurves());
    const a = r.at("position.x", 1);
    down(r.canvas, a);
    move(r.canvas, { clientX: a.clientX + 80, clientY: a.clientY });
    fireEvent.keyDown(r.canvas, { key: "Escape" });
    expect(r.events.gesture).toHaveBeenLastCalledWith("cancel", null);
    up(r.canvas, { clientX: a.clientX + 80, clientY: a.clientY });
    expect(r.events.gesture.mock.calls.filter((c) => c[0] === "commit")).toHaveLength(0);
  });

  it("moves every selected curve by the same share in the normalized view", () => {
    const r = rig(positionCurves());
    r.view.setNormalized(true);
    r.view.setViewport(null);
    r.tick();
    r.view.setSelection([
      { curve: "position.x", index: 1 },
      { curve: "position.y", index: 1 },
    ]);
    const vp = r.view.viewport;
    if (vp === null) throw new Error("no viewport");
    const plot = r.view.plot();
    // Normalized: x's key at 200 of -400..200 is at 1, y's at -150 of -150..300 is at 0.
    const x = xOf(vp, plot, 3000);
    const y = yOf(vp, plot, 1);
    down(r.canvas, { clientX: x, clientY: y });
    move(r.canvas, { clientX: x, clientY: y + 100 });
    const edit = lastEdit(r) as { dv: Record<string, number> };
    const share = (-100 / plot.height) * (vp.v1 - vp.v0);
    expect(edit.dv["position.x"]).toBeCloseTo(share * 600, 0);
    expect(edit.dv["position.y"]).toBeCloseTo(share * 450, 0);
  });
});

describe("bezier handles", () => {
  function handleAt(r: Rig, id: string, index: number, side: "out" | "in") {
    const curve = r.view.curves.find((c) => c.id === id) as GraphCurve;
    const seg = segmentOf(curve, side === "out" ? index : index - 1);
    const interp = curve.interps[side === "out" ? index : index - 1];
    if (interp?.kind !== "bezier") throw new Error("not a bezier");
    const p = handlePoint(seg, interp, side);
    const vp = r.view.viewport;
    if (vp === null) throw new Error("no viewport");
    return { clientX: xOf(vp, r.view.plot(), p.t), clientY: yOf(vp, r.view.plot(), p.v) };
  }

  it("shows the handles of a selected key and turns a drag into bezier numbers", () => {
    const r = rig(positionCurves());
    r.view.setSelection([{ curve: "position.x", index: 1 }]);
    r.tick();
    expect(r.calls.arc).toBe(2); // the in and the out handle of the middle key
    const vp = r.view.viewport;
    if (vp === null) throw new Error("no viewport");
    const plot = r.view.plot();
    const from = handleAt(r, "position.x", 1, "out");
    // To a quarter of the way to the next key in time, and half of the way in value.
    const to = { clientX: xOf(vp, plot, 3500), clientY: yOf(vp, plot, 100) };
    down(r.canvas, from);
    move(r.canvas, to);
    const edit = lastEdit(r) as Extract<GraphEdit, { type: "tangent" }>;
    expect(edit).toMatchObject({ type: "tangent", curve: "position.x", index: 1, side: "out" });
    expect(edit.x).toBeCloseTo(0.25, 3);
    expect(edit.y).toBeCloseTo(0.5, 3);
    expect(edit.opposite).toBeNull(); // free
    up(r.canvas, to);
    expect(r.events.gesture).toHaveBeenLastCalledWith("commit", edit);
    expect(r.events.select).not.toHaveBeenCalled();
  });

  it("aligned and mirrored move the handle across the key; Alt frees it", () => {
    const r = rig(positionCurves());
    r.view.setSelection([{ curve: "position.x", index: 1 }]);
    r.view.setTangentMode("mirrored");
    const vp = r.view.viewport;
    if (vp === null) throw new Error("no viewport");
    const plot = r.view.plot();
    const from = handleAt(r, "position.x", 1, "out");
    const to = { clientX: xOf(vp, plot, 3500), clientY: yOf(vp, plot, 100) };
    down(r.canvas, from);
    move(r.canvas, to);
    // Out: 500 later, 100 lower. Mirrored in: 500 earlier, 100 higher, of a segment rising 600.
    const mirrored = lastEdit(r) as Extract<GraphEdit, { type: "tangent" }>;
    expect(mirrored.opposite?.x).toBeCloseTo(0.75, 3);
    expect(mirrored.opposite?.y).toBeCloseTo(1 + 100 / 600, 3);
    move(r.canvas, { ...to, altKey: true });
    expect((lastEdit(r) as { opposite: unknown }).opposite).toBeNull();
    up(r.canvas, to);

    r.view.setTangentMode("aligned");
    down(r.canvas, from);
    move(r.canvas, to);
    // Aligned keeps the in handle's reach (0.42 of 2000 back) on the out handle's slope (-0.2).
    const aligned = lastEdit(r) as Extract<GraphEdit, { type: "tangent" }>;
    expect(aligned.opposite?.x).toBeCloseTo(0.58, 3);
    expect(aligned.opposite?.y).toBeCloseTo(1 + (0.2 * 840) / 600, 3);
  });

  it("has no handle to drag on a linear segment", () => {
    const r = rig(positionCurves());
    r.view.setSelection([{ curve: "position.x", index: 2 }]);
    r.tick();
    expect(r.calls.arc).toBe(1); // the last key: only the handle arriving from the eased segment
  });
});

describe("pan, zoom and fit", () => {
  it("zooms time about the pointer with Ctrl+wheel and tells the host", () => {
    const r = rig(positionCurves());
    const before = r.view.viewport;
    if (before === null) throw new Error("no viewport");
    const plot = r.view.plot();
    const t = tAt(before, plot, 300);
    fireEvent.wheel(r.canvas, { clientX: 300, clientY: 200, deltaY: -100, ctrlKey: true });
    const after = r.view.viewport;
    if (after === null) throw new Error("no viewport");
    expect(after.t1 - after.t0).toBeLessThan(before.t1 - before.t0);
    expect(after.v1 - after.v0).toBe(before.v1 - before.v0);
    expect(tAt(after, plot, 300)).toBeCloseTo(t, 6);
    expect(r.events.viewport).toHaveBeenLastCalledWith(after);
  });

  it("pans with the wheel and with a middle-button drag", () => {
    const r = rig(positionCurves());
    const before = r.view.viewport;
    if (before === null) throw new Error("no viewport");
    fireEvent.wheel(r.canvas, { clientX: 300, clientY: 200, deltaY: 40 });
    expect(r.view.viewport?.v0).toBeLessThan(before.v0);
    expect(r.view.viewport?.t0).toBe(before.t0);
    fireEvent.pointerDown(r.canvas, { button: 1, pointerId: 1, clientX: 300, clientY: 200 });
    move(r.canvas, { clientX: 350, clientY: 200 });
    fireEvent.pointerUp(r.canvas, { button: 1, pointerId: 1, clientX: 350, clientY: 200 });
    expect(r.view.viewport?.t0).toBeLessThan(before.t0);
    expect(r.events.select).not.toHaveBeenCalled();
  });

  it("fits every key with F and the selected keys with Shift+F", () => {
    const r = rig(positionCurves());
    const all = r.view.viewport;
    r.view.setViewport({ t0: 0, t1: 10, v0: 0, v1: 1 });
    fireEvent.keyDown(r.canvas, { key: "f" });
    expect(r.view.viewport).toEqual(all);
    r.view.setSelection([
      { curve: "position.x", index: 0 },
      { curve: "position.x", index: 1 },
    ]);
    fireEvent.keyDown(r.canvas, { key: "F", shiftKey: true });
    expect(r.view.viewport?.t1).toBeLessThan(5000);
    expect(r.view.viewport?.t0).toBeLessThan(1000);
  });

  it("moves the playhead from the time ruler", () => {
    const r = rig(positionCurves());
    const vp = r.view.viewport;
    if (vp === null) throw new Error("no viewport");
    const x = xOf(vp, r.view.plot(), 2000);
    down(r.canvas, { clientX: x, clientY: 10 });
    expect(r.events.scrub.mock.calls[0]?.[0]).toBeCloseTo(2000, 6);
    move(r.canvas, { clientX: xOf(vp, r.view.plot(), 2600), clientY: 200 });
    expect(r.events.scrub.mock.calls[1]?.[0]).toBeCloseTo(2600, 6);
    up(r.canvas, { clientX: x, clientY: 10 });
    expect(r.events.select).not.toHaveBeenCalled();
  });
});

describe("the keyboard and the mouse's other buttons", () => {
  it("nudges with the arrows, steps with Alt, deletes, opens the easing, selects all", () => {
    const r = rig(positionCurves());
    r.view.setSelection([{ curve: "position.x", index: 1 }]);
    fireEvent.keyDown(r.canvas, { key: "ArrowRight" });
    expect(r.events.nudge).toHaveBeenLastCalledWith(1, 0);
    fireEvent.keyDown(r.canvas, { key: "ArrowLeft", shiftKey: true });
    expect(r.events.nudge).toHaveBeenLastCalledWith(-10, 0);
    fireEvent.keyDown(r.canvas, { key: "ArrowUp" });
    expect(r.events.nudge).toHaveBeenLastCalledWith(0, 1);
    fireEvent.keyDown(r.canvas, { key: "ArrowDown", shiftKey: true });
    expect(r.events.nudge).toHaveBeenLastCalledWith(0, -10);
    fireEvent.keyDown(r.canvas, { key: "ArrowRight", altKey: true });
    expect(r.events.action).toHaveBeenLastCalledWith("nextKey");
    fireEvent.keyDown(r.canvas, { key: "ArrowLeft", altKey: true });
    expect(r.events.action).toHaveBeenLastCalledWith("previousKey");
    fireEvent.keyDown(r.canvas, { key: "Delete" });
    expect(r.events.action).toHaveBeenLastCalledWith("delete");
    fireEvent.keyDown(r.canvas, { key: "Enter" });
    expect(r.events.action).toHaveBeenLastCalledWith("easing");
    fireEvent.keyDown(r.canvas, { key: "a", ctrlKey: true });
    expect(r.events.action).toHaveBeenLastCalledWith("selectAll");
    fireEvent.keyDown(r.canvas, { key: "Escape" });
    expect(r.events.action).toHaveBeenLastCalledWith("clear");
  });

  it("leaves the keys it does not use to the app (undo, Space, the arrows with no key selected)", () => {
    const r = rig(positionCurves());
    const seen = vi.fn();
    document.body.addEventListener("keydown", seen);
    fireEvent.keyDown(r.canvas, { key: "z", ctrlKey: true });
    fireEvent.keyDown(r.canvas, { key: " " });
    fireEvent.keyDown(r.canvas, { key: "ArrowRight" }); // nothing selected: the app's frame step
    expect(seen).toHaveBeenCalledTimes(3);
    expect(r.events.nudge).not.toHaveBeenCalled();
    r.view.setSelection([{ curve: "position.x", index: 1 }]);
    fireEvent.keyDown(r.canvas, { key: "ArrowRight" }); // a nudge: stops here
    document.body.removeEventListener("keydown", seen);
    expect(seen).toHaveBeenCalledTimes(3);
    expect(r.events.nudge).toHaveBeenCalledTimes(1);
  });

  it("adds a key on a double-click on a curve, and says which key is under a point", () => {
    const r = rig(positionCurves());
    const vp = r.view.viewport;
    if (vp === null) throw new Error("no viewport");
    const plot = r.view.plot();
    // Half way along the first segment of Position Y, which is eased: on the curve.
    const out = new Float64Array(2);
    (r.view.curves[1] as GraphCurve).sample(2000, 2001, 2, out);
    const p = { clientX: xOf(vp, plot, 2000), clientY: yOf(vp, plot, out[0] as number) };
    fireEvent.dblClick(r.canvas, p);
    expect(r.events.add.mock.calls[0]?.[0]).toBe("position.y");
    expect(r.events.add.mock.calls[0]?.[1]).toBeCloseTo(2000, 6);
    const key = r.at("position.x", 0);
    expect(r.view.keyAtClient(key.clientX, key.clientY)).toEqual({
      curve: "position.x",
      index: 0,
    });
    expect(r.view.keyAtClient(key.clientX + 40, key.clientY - 40)).toBeNull();
    expect(r.view.selectedAtClient()).toBeNull();
    r.view.setSelection([{ curve: "position.x", index: 0 }]);
    expect(r.view.selectedAtClient()).toEqual({
      ref: { curve: "position.x", index: 0 },
      ...key,
    });
  });
});

describe("<GraphEditor>", () => {
  it("hands the view to its host, takes data as props, and passes axe", async () => {
    const proto = HTMLCanvasElement.prototype;
    const original = proto.getContext;
    proto.getContext = (() => countingContext().ctx) as unknown as typeof proto.getContext;
    try {
      let handle: GraphEditorHandle | null = null;
      const curves = positionCurves();
      const { container } = render(
        h(GraphEditor, {
          options: FIXTURE_OPTIONS,
          curves,
          selection: [{ curve: "position.x", index: 1 }],
          playhead: 2000,
          theme: fixtureTheme(),
          onView: (got) => {
            handle = got;
          },
        }),
      );
      const got = handle as GraphEditorHandle | null;
      expect(got?.view.curves).toBe(curves);
      const canvas = container.querySelector("canvas") as HTMLCanvasElement;
      expect(canvas.getAttribute("role")).toBe("application");
      expect(canvas.getAttribute("aria-label")).toBe("Graph editor");
      expect(canvas.tabIndex).toBe(0);
      got?.announce("Position X, key 2 of 3");
      expect(container.querySelector('[role="status"]')?.textContent).toBe(
        "Position X, key 2 of 3",
      );
      // No element per key or per curve: the canvas, its description and the status line.
      expect(container.querySelectorAll("*")).toHaveLength(4);
      const results = await axe.run(container);
      expect(results.violations.map((v) => v.id)).toEqual([]);
    } finally {
      proto.getContext = original;
    }
  });
});
