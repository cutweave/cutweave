// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// The preview's handles (WP-2H.06): pointer drags and keys become `item.set_props` calls inside one
// gesture or batch transaction, in canvas units at any preview zoom, keyed at the playhead. The
// engine and the drawn picture are covered by the user flow (F45); here the engine is a recorder.
import { TooltipProvider } from "@cutweave/ui";
import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { createElement as h } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../i18n/index.ts";
import { docStore } from "../../state/docStore.ts";
import { initialViewState, viewStore } from "../../state/viewStore.ts";
import { createFakeEngine, type FakeEngine } from "../../testing/fakeEngine.ts";
import { sampleProject } from "../../testing/fixtures.ts";
import { guideStore, initialGuides } from "./guides.ts";
import { TransformOverlay } from "./TransformOverlay.tsx";

/** The overlay box on screen: a 1920 x 1080 canvas shown at `zoom`. */
let zoom = 0.5;
let engine: FakeEngine;

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  // jsdom has no layout and no pointer capture.
  Element.prototype.setPointerCapture = () => {};
  Element.prototype.releasePointerCapture = () => {};
  Element.prototype.hasPointerCapture = () => false;
  Element.prototype.getBoundingClientRect = function () {
    const box = this.classList?.contains("cw-xform") === true;
    const width = box ? 1920 * zoom : 0;
    const height = box ? 1080 * zoom : 0;
    return { left: 0, top: 0, right: width, bottom: height, width, height, x: 0, y: 0 } as DOMRect;
  };
});

function load(clip: object = {}, track: object = {}) {
  const project = sampleProject({
    itm_v: { start: 0, visual: visual(clip) },
  });
  const trackV = (project.sequences as Record<string, { tracks: Record<string, object> }>).seq_1
    ?.tracks.trk_v;
  Object.assign(trackV ?? {}, track);
  docStore.getState().resync({ rev: 1, committed_rev: 1, project });
}

const visual = (over: object) => ({
  transform: {
    position: { x: 0, y: 0 },
    scale: { x: 1, y: 1 },
    rotation: 0,
    anchor: { x: 0.5, y: 0.5 },
  },
  opacity: 1,
  crop: { left: 0, top: 0, right: 0, bottom: 0 },
  flip: { horizontal: false, vertical: false },
  blend_mode: "normal",
  ...over,
});

const mount = () =>
  render(h(I18nextProvider, { i18n }, h(TooltipProvider, null, h(TransformOverlay))));

const handle = (name: string) => {
  const el = document.querySelector<HTMLElement>(`.cw-xform__handle[data-handle="${name}"]`);
  if (el === null) throw new Error(`no ${name} handle`);
  return el;
};
const body = () => {
  const el = document.querySelector<SVGElement>(".cw-xform__body");
  if (el === null) throw new Error("no body");
  return el;
};
const root = () => {
  const el = document.querySelector<HTMLElement>(".cw-xform");
  if (el === null) throw new Error("no overlay");
  return el;
};

const down = (el: Element, x: number, y: number) =>
  fireEvent.pointerDown(el, { clientX: x, clientY: y, button: 0, pointerId: 1 });
const moveTo = (el: Element, x: number, y: number, init: object = {}) =>
  fireEvent.pointerMove(el, { clientX: x, clientY: y, pointerId: 1, ...init });
const up = (el: Element) => fireEvent.pointerUp(el, { pointerId: 1 });

const setProps = () => engine.calls.filter((c) => c.name === "item.set_props");
const lastProps = () => {
  const call = setProps().at(-1);
  if (call === undefined) throw new Error("no item.set_props");
  return call.input as {
    item: string;
    at: number;
    props: {
      transform: {
        position: { x: number; y: number } | null;
        scale: { x: number; y: number } | null;
        rotation: number | null;
      } | null;
      crop: { left: number | null; right: number | null } | null;
    };
  };
};
const settle = () => vi.waitFor(() => expect(engine.names()).toContain("edit.commit_tx"));

beforeEach(() => {
  zoom = 0.5;
  viewStore.setState({
    ...initialViewState,
    layouts: {},
    playhead: 0,
    selection: [{ kind: "item", id: "itm_v" as never }],
  });
  guideStore.setState({ ...initialGuides });
  engine = createFakeEngine();
  engine.install();
});
afterEach(() => {
  cleanup();
  engine.uninstall();
});

describe("TransformOverlay", () => {
  it("draws nothing for no selection and no handles on a locked track", () => {
    load();
    viewStore.setState({ selection: [] });
    mount();
    expect(document.querySelector(".cw-xform")).toBeNull();
    cleanup();
    viewStore.setState({ selection: [{ kind: "item", id: "itm_v" as never }] });
    load({}, { locked: true });
    mount();
    expect(document.querySelector(".cw-xform__outline")).not.toBeNull();
    expect(document.querySelector(".cw-xform__handle")).toBeNull();
  });

  it("dragging the picture moves it by the pointer's distance in sequence px at any zoom", async () => {
    for (const z of [0.25, 0.5, 1]) {
      zoom = z;
      engine.calls.length = 0;
      load();
      const view = mount();
      // 100 sequence px to the right, whatever the zoom.
      down(body(), 480, 270);
      act(() => moveTo(body(), 480 + 100 * z, 270));
      act(() => up(body()));
      await settle();
      const input = lastProps();
      expect((input.props.transform?.position?.x ?? 0) * 1920).toBeCloseTo(100, 6);
      expect(input.props.transform?.position?.y).toBe(0);
      // One gesture: begin, updates with its transaction, one commit.
      expect(engine.names().filter((n) => n === "edit.begin_tx")).toHaveLength(1);
      expect(engine.calls.find((c) => c.name === "edit.begin_tx")?.input).toMatchObject({
        kind: "gesture",
        label: "Move clip in the picture",
      });
      expect(setProps().every((c) => (c.meta as { tx: string }).tx.startsWith("tx_"))).toBe(true);
      expect(input.at).toBe(0);
      expect(engine.names().filter((n) => n === "edit.commit_tx")).toHaveLength(1);
      view.unmount();
    }
  });

  it("Escape in the middle of a drag rolls everything back", async () => {
    load();
    mount();
    down(body(), 480, 270);
    act(() => moveTo(body(), 530, 270));
    await vi.waitFor(() => expect(setProps().length).toBeGreaterThan(0));
    act(() => {
      fireEvent.keyDown(root(), { key: "Escape" });
    });
    await vi.waitFor(() => expect(engine.names()).toContain("edit.cancel_tx"));
    expect(engine.names()).not.toContain("edit.commit_tx");
  });

  it("a corner scales uniformly with the opposite corner fixed", async () => {
    load();
    mount();
    // The bottom-right corner sits at (960, 540) css; drag it to the canvas centre.
    down(handle("br"), 960, 540);
    act(() => moveTo(handle("br"), 480, 270));
    act(() => up(handle("br")));
    await settle();
    const { transform } = lastProps().props;
    expect(transform?.scale?.x).toBeCloseTo(0.5, 6);
    expect(transform?.scale?.y).toBeCloseTo(0.5, 6);
    // The top-left corner stayed at the canvas's: the centre moved to a quarter of each side.
    expect(transform?.position?.x).toBeCloseTo(-0.25, 6);
    expect(transform?.position?.y).toBeCloseTo(-0.25, 6);
  });

  it("Shift frees the corner", async () => {
    load();
    mount();
    down(handle("br"), 960, 540);
    act(() => moveTo(handle("br"), 480, 540, { shiftKey: true }));
    act(() => up(handle("br")));
    await settle();
    const { scale } = lastProps().props.transform ?? {};
    expect(scale?.x).toBeCloseTo(0.5, 6);
    expect(scale?.y).toBeCloseTo(1, 6);
  });

  it("the rotate handle turns the clip about its anchor, 15 degrees to within 0.5", async () => {
    load();
    mount();
    // Grab the handle, then sweep the pointer 15 degrees about the picture's centre (480, 270).
    const stand = handle("rotate").style;
    const hx = (Number.parseFloat(stand.left) / 100) * 1920 * zoom;
    const hy = (Number.parseFloat(stand.top) / 100) * 1080 * zoom;
    down(handle("rotate"), hx, hy);
    const radius = Math.hypot(hx - 480, hy - 270);
    const start = Math.atan2(hy - 270, hx - 480);
    const to = start + (15 * Math.PI) / 180;
    act(() => moveTo(handle("rotate"), 480 + radius * Math.cos(to), 270 + radius * Math.sin(to)));
    act(() => up(handle("rotate")));
    await settle();
    expect(lastProps().props.transform?.rotation).toBeCloseTo(15, 6);
  });

  it("crop mode: an edge handle crops that edge and the picture stays where it is", async () => {
    load();
    guideStore.getState().set({ cropMode: true });
    mount();
    down(handle("l"), 0, 270);
    act(() => moveTo(handle("l"), 240, 270));
    act(() => up(handle("l")));
    await settle();
    const { props } = lastProps();
    expect(props.crop?.left).toBeCloseTo(0.25, 6);
    expect(props.transform).toBeNull();
    expect(document.querySelector(".cw-xform__handle[data-kind='rotate']")).toBeNull();
  });

  it("crop on a horizontally flipped clip: its visible left edge is crop.right", async () => {
    load({ flip: { horizontal: true, vertical: false } });
    guideStore.getState().set({ cropMode: true });
    mount();
    down(handle("l"), 0, 270);
    act(() => moveTo(handle("l"), 240, 270));
    act(() => up(handle("l")));
    await settle();
    expect(lastProps().props.crop?.right).toBeCloseTo(0.25, 6);
    expect(lastProps().props.crop?.left).toBe(0);
  });

  it("Shift-dragging a crop edge crops the opposite edge by the same amount", async () => {
    load();
    guideStore.getState().set({ cropMode: true });
    mount();
    down(handle("l"), 0, 270);
    act(() => moveTo(handle("l"), 240, 270, { shiftKey: true }));
    act(() => up(handle("l")));
    await settle();
    expect(lastProps().props.crop).toMatchObject({ left: 0.25, right: 0.25 });
  });

  it("snaps the picture's edge to the canvas centre line and halves for side-by-side", async () => {
    load({ transform: { ...visual({}).transform, scale: { x: 0.5, y: 0.5 } } });
    mount();
    // A half-size clip centred: its right edge is at 1440. Drag it left so the edge is 4 px off
    // the centre line (960): it snaps onto it.
    down(body(), 480, 270);
    act(() => moveTo(body(), 480 - (1440 - 964) * 0.5, 270));
    act(() => up(body()));
    await settle();
    expect((lastProps().props.transform?.position?.x ?? 0) * 1920).toBeCloseTo(-480, 6);
  });

  it("arrow keys nudge one sequence px, Shift ten, as one undo step each", async () => {
    load();
    mount();
    act(() => {
      fireEvent.keyDown(root(), { key: "ArrowRight" });
    });
    await vi.waitFor(() => expect(engine.names()).toContain("edit.commit_tx"));
    expect((lastProps().props.transform?.position?.x ?? 0) * 1920).toBeCloseTo(1, 6);
    expect(engine.calls.find((c) => c.name === "edit.begin_tx")?.input).toMatchObject({
      kind: "batch",
    });
    engine.calls.length = 0;
    act(() => {
      fireEvent.keyDown(root(), { key: "ArrowUp", shiftKey: true });
    });
    await vi.waitFor(() => expect(engine.names()).toContain("edit.commit_tx"));
    expect((lastProps().props.transform?.position?.y ?? 0) * 1080).toBeCloseTo(-10, 6);
  });

  it("C switches to crop handles and Escape back", () => {
    load();
    mount();
    act(() => {
      fireEvent.keyDown(root(), { key: "c" });
    });
    expect(guideStore.getState().cropMode).toBe(true);
    act(() => {
      fireEvent.keyDown(root(), { key: "Escape" });
    });
    expect(guideStore.getState().cropMode).toBe(false);
  });

  it("an animated property is keyed at the playhead (the item time of `at`)", async () => {
    load({
      transform: {
        ...visual({}).transform,
        position: {
          keyframes: [
            { t: 0, value: { x: 0, y: 0 }, interp: { kind: "linear" } },
            { t: 705_600_000 * 4, value: { x: 0.4, y: 0 }, interp: { kind: "linear" } },
          ],
        },
      },
    });
    viewStore.setState({ playhead: 705_600_000 * 2 });
    mount();
    // At the half-way point the picture is 0.2 of the canvas to the right; drag it 130 sequence px.
    down(body(), 480, 270);
    act(() => moveTo(body(), 545, 270));
    act(() => up(body()));
    await settle();
    const input = lastProps();
    expect(input.at).toBe(705_600_000 * 2);
    expect((input.props.transform?.position?.x ?? 0) * 1920).toBeCloseTo(0.2 * 1920 + 130, 4);
  });
});
