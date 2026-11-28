// SPDX-License-Identifier: GPL-3.0-or-later
// @vitest-environment jsdom
// WP-2C.07: the Graph Editor panel. The performance rule first (issues #241 and #244): while the
// panel is closed nothing of the graph editor runs on a document patch; while it is open a clip
// drag redraws the canvas and renders no React. Then what the shell shows and sends: the curve
// list, the interpolation switch, the typed Time and Value, Delete, and axe.
import type { DocPatched, PatchOp, Project } from "@cutweave/protocol";
import { mathModuleBytes } from "@cutweave/protocol/test-support/math-wasm";
import { anim, initMath } from "@cutweave/protocol/wasm";
import { type GraphEditorHandle, type GraphTheme, TooltipProvider } from "@cutweave/ui";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import axe from "axe-core";
import { createElement as h, Profiler } from "react";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../i18n/index.ts";
import { docStore } from "../../state/docStore.ts";
import { initialPlaybackState, playbackStore } from "../../state/playbackStore.ts";
import { initialViewState, viewStore } from "../../state/viewStore.ts";
import { createFakeEngine, type FakeEngine } from "../../testing/fakeEngine.ts";
import { sampleProject, sec } from "../../testing/fixtures.ts";
import { EMPTY_KEY_SELECTION, keySelectionStore, selectKeys } from "../../timeline/keyframes.ts";
import { GraphEditorPanel } from "./GraphEditorPanel.tsx";
import {
  graphShellStore,
  graphViewStore,
  graphWork,
  initialGraphShell,
  initialGraphView,
} from "./graphStore.ts";
import { keyedProject } from "./testing.ts";

const seen = vi.hoisted(() => ({ handle: null as unknown }));

// The canvas gets fixed tokens (jsdom has no stylesheet), and the test keeps its view.
vi.mock("@cutweave/ui", async (importOriginal) => {
  const ui = await importOriginal<typeof import("@cutweave/ui")>();
  const { createElement } = await import("react");
  const theme: GraphTheme = {
    surface: "black",
    ruler: "black",
    grid: "gray",
    gridStrong: "gray",
    text: "white",
    textStrong: "white",
    playhead: "red",
    selected: "orange",
    onSelected: "black",
    marquee: "orange",
    guide: "white",
    curves: ["blue", "orange", "green"],
    font: "12px monospace",
    fontPx: 12,
    dimmed: 0.5,
    line: 1,
    lineStrong: 2,
    rulerHeight: 24,
    rulerWidth: 64,
    keySize: 8,
    handleSize: 6,
    hitRadius: 8,
    pad: 4,
  };
  return {
    ...ui,
    GraphEditor: (props: Parameters<typeof ui.GraphEditor>[0]) =>
      createElement(ui.GraphEditor, {
        ...props,
        theme,
        onView: (handle: GraphEditorHandle | null) => {
          seen.handle = handle;
          props.onView?.(handle);
        },
      }),
  };
});

const UPDATES = 5;

const patch = (rev: number, path: string, old: unknown, value: unknown): DocPatched =>
  ({
    rev,
    prev_rev: rev - 1,
    committed_rev: 1,
    actor: "user",
    tx_id: "tx_drag",
    cause: "command",
    request_id: null,
    ops: [{ op: "replace", path, old, value }] as unknown as PatchOp[],
  }) as unknown as DocPatched;

let rev = 1;

/** `n` drag updates of a clip: the engine moved it (`item.move` inside a gesture). */
function dragClip(id: string, track: string, n = UPDATES): void {
  let at = sec(1);
  for (let i = 0; i < n; i++) {
    const next = at + sec(0.1);
    rev += 1;
    const event = patch(rev, `/sequences/seq_1/tracks/${track}/items/${id}/start`, at, next);
    act(() => {
      expect(docStore.getState().applyEvent(event)).toBe("applied");
    });
    at = next;
  }
}

const load = (project: Project): void => {
  rev = 1;
  docStore.getState().resync({ rev: 1, committed_rev: 1, project });
};

let engine: FakeEngine;
const noop = new Proxy(
  {},
  {
    get: (_t, prop) => (prop === "measureText" ? () => ({ width: 0 }) : () => undefined),
    set: () => true,
  },
) as CanvasRenderingContext2D;

beforeAll(async () => {
  await initMath(mathModuleBytes());
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollIntoView ??= () => {};
  Element.prototype.hasPointerCapture ??= () => false;
  Element.prototype.setPointerCapture ??= () => {};
  Element.prototype.releasePointerCapture ??= () => {};
  HTMLCanvasElement.prototype.getContext = (() =>
    noop) as unknown as HTMLCanvasElement["getContext"];
}, 900_000);

beforeEach(() => {
  viewStore.setState({ ...initialViewState, layouts: {} });
  playbackStore.setState({ ...initialPlaybackState });
  docStore.setState({ status: "empty", rev: 0, committedRev: 0, project: null });
  keySelectionStore.setState(EMPTY_KEY_SELECTION, true);
  graphViewStore.setState(initialGraphView, true);
  graphShellStore.setState(initialGraphShell, true);
  seen.handle = null;
  engine = createFakeEngine();
  engine.install();
  load(keyedProject());
  viewStore.setState({ selection: [{ kind: "item", id: "itm_v" }] } as never);
});

afterEach(() => {
  cleanup();
  engine.uninstall();
});

/** Mounts the panel inside a Profiler; `commits.n` counts its React commits. */
function mount() {
  const commits = { n: 0 };
  const view = render(
    h(
      I18nextProvider,
      { i18n },
      h(
        TooltipProvider,
        null,
        h(
          Profiler,
          {
            id: "graph",
            onRender: () => {
              commits.n += 1;
            },
          },
          h(GraphEditorPanel),
        ),
      ),
    ),
  );
  const handle = seen.handle as GraphEditorHandle;
  // jsdom lays nothing out: give the canvas a size as the ResizeObserver would.
  act(() => handle.view.resize(864, 424, 1));
  return { commits, handle, ...view };
}

/** Lets the engine calls of an edit finish. */
const settle = async (): Promise<void> => {
  await act(async () => {
    for (let i = 0; i < 30; i++) await new Promise((resolve) => setTimeout(resolve, 0));
  });
};

describe("the performance rule", () => {
  it("does no graph work for 100 document patches while the panel is closed", () => {
    const compiled = vi.spyOn(anim, "compile");
    const rebuilds = graphWork.rebuilds;
    const shell = graphShellStore.getState();
    dragClip("itm_v", "trk_v", 100);
    expect(graphWork.rebuilds - rebuilds).toBe(0);
    expect(compiled).not.toHaveBeenCalled();
    expect(graphShellStore.getState()).toBe(shell);
    compiled.mockRestore();
  });

  it("does none after the panel was opened and closed again", () => {
    const { unmount } = mount();
    unmount();
    const compiled = vi.spyOn(anim, "compile");
    const rebuilds = graphWork.rebuilds;
    const shell = graphShellStore.getState();
    dragClip("itm_v", "trk_v", 100);
    act(() => playbackStore.getState().setPlayhead(sec(2)));
    act(() => selectKeys("itm_v", [{ prop: "opacity", t: 0 }]));
    expect(graphWork.rebuilds - rebuilds).toBe(0);
    expect(compiled).not.toHaveBeenCalled();
    expect(graphShellStore.getState()).toBe(shell);
    compiled.mockRestore();
  });

  it("redraws the canvas and renders no React while a clip is dragged with the panel open", () => {
    const { commits, handle } = mount();
    expect(handle.view.curves).toHaveLength(3);
    handle.view.flush();
    const before = commits.n;
    const frames = handle.view.stats.frames;
    const rebuilds = graphWork.rebuilds;
    const times = [...(handle.view.curves[0]?.times as Float64Array)];
    dragClip("itm_v", "trk_v");
    // The panel's shell (toolbar, curve list, the canvas element) did not render...
    expect(commits.n - before).toBe(0);
    // ...the curves followed the clip (its keys are half a second later on the sequence)...
    expect(graphWork.rebuilds - rebuilds).toBe(UPDATES);
    expect([...(handle.view.curves[0]?.times as Float64Array)]).toEqual(
      times.map((t) => t + UPDATES * sec(0.1)),
    );
    // ...and the canvas draws them, once however many updates a frame holds.
    handle.view.flush();
    expect(handle.view.stats.frames - frames).toBe(1);
  });

  it("neither rebuilds nor draws when another clip is dragged", () => {
    const { commits, handle } = mount();
    handle.view.flush();
    const before = commits.n;
    const frames = handle.view.stats.frames;
    const rebuilds = graphWork.rebuilds;
    const curves = handle.view.curves;
    dragClip("itm_a", "trk_a");
    handle.view.flush();
    expect(commits.n - before).toBe(0);
    expect(graphWork.rebuilds - rebuilds).toBe(0);
    expect(handle.view.curves).toBe(curves);
    expect(handle.view.stats.frames - frames).toBe(0);
  });

  it("moves the playhead on the canvas without rendering", () => {
    const { commits, handle } = mount();
    handle.view.flush();
    const before = commits.n;
    const frames = handle.view.stats.frames;
    for (let i = 1; i <= 10; i++) act(() => playbackStore.getState().setPlayhead(sec(1 + i / 10)));
    handle.view.flush();
    expect(commits.n - before).toBe(0);
    expect(handle.view.stats.frames - frames).toBe(1);
  });

  it("pans and zooms without rendering, and keeps the viewport for the next time it opens", () => {
    const { commits, handle, unmount, container } = mount();
    handle.view.flush();
    const before = commits.n;
    const canvas = container.querySelector("canvas") as HTMLCanvasElement;
    for (let i = 0; i < 20; i++) {
      fireEvent.wheel(canvas, { clientX: 300, clientY: 200, deltaY: -100, ctrlKey: true });
      fireEvent.wheel(canvas, { clientX: 300, clientY: 200, deltaY: 30 });
    }
    expect(commits.n - before).toBe(0);
    const viewport = handle.view.viewport;
    expect(graphViewStore.getState().viewport).toEqual(viewport);
    unmount();
    const again = mount();
    expect(again.handle.view.viewport).toEqual(viewport);
  });
});

describe("the panel", () => {
  it("lists the animated properties and hides one from its eye", () => {
    const { handle } = mount();
    const list = screen.getByRole("list", { name: "Curves" });
    expect(list.textContent).toContain("Position");
    expect(list.textContent).toContain("Opacity");
    fireEvent.click(screen.getByRole("button", { name: "Hide Position" }));
    expect(handle.view.curves.map((c) => c.label)).toEqual(["Opacity"]);
    expect(graphViewStore.getState().hidden).toEqual(["transform.position"]);
    fireEvent.click(screen.getByRole("button", { name: "Show Position" }));
    expect(handle.view.curves).toHaveLength(3);
  });

  it("says so when no selected clip is animated, and follows the selection", () => {
    load(sampleProject());
    mount();
    expect(screen.getByText("No animated properties")).toBeTruthy();
    act(() => load(keyedProject()));
    expect(screen.queryByText("No animated properties")).toBeNull();
    act(() => viewStore.setState({ selection: [] } as never));
    expect(screen.getByText("No animated properties")).toBeTruthy();
  });

  it("enables the tools for a selected key and shows its time and value", () => {
    mount();
    const hold = screen.getByRole("button", { name: "Hold" });
    const time = screen.getByRole("textbox", { name: "Time" }) as HTMLInputElement;
    const value = screen.getByRole("textbox", { name: "Value" }) as HTMLInputElement;
    expect((hold as HTMLButtonElement).disabled).toBe(true);
    expect(time.disabled).toBe(true);
    act(() => selectKeys("itm_v", [{ prop: "opacity", t: sec(2) }]));
    expect((hold as HTMLButtonElement).disabled).toBe(false);
    // The clip starts at 1 s: the key at 2 s of the clip is at 3 s of the sequence (30 fps).
    expect(time.value).toBe("00:00:03:00");
    expect(value.value).toBe("0.8");
    expect(screen.getByRole("button", { name: "Linear" }).getAttribute("aria-pressed")).toBe(
      "true",
    );
  });

  it("sends a typed value as one gesture transaction of keyframe.set", async () => {
    mount();
    act(() => selectKeys("itm_v", [{ prop: "opacity", t: sec(2) }]));
    const value = screen.getByRole("textbox", { name: "Value" });
    fireEvent.change(value, { target: { value: "0.25" } });
    fireEvent.keyDown(value, { key: "Enter" });
    await settle();
    expect(engine.calls.map((c) => [c.name, c.input, c.meta])).toEqual([
      ["edit.begin_tx", { kind: "gesture", label: "Change keyframe value" }, undefined],
      [
        "keyframe.set",
        { item: "itm_v", prop: "opacity", keys: [{ t: sec(2), value: 0.25 }] },
        { tx: expect.any(String), replace: true },
      ],
      ["edit.commit_tx", { tx: expect.any(String) }, undefined],
    ]);
  });

  it("moves the key to a typed time and refuses text that is not a time", async () => {
    mount();
    act(() => selectKeys("itm_v", [{ prop: "opacity", t: sec(2) }]));
    const time = screen.getByRole("textbox", { name: "Time" }) as HTMLInputElement;
    fireEvent.change(time, { target: { value: "soon" } });
    fireEvent.keyDown(time, { key: "Enter" });
    expect(time.getAttribute("aria-invalid")).toBe("true");
    expect(engine.calls).toEqual([]);
    fireEvent.change(time, { target: { value: "00:00:02:15" } });
    fireEvent.keyDown(time, { key: "Enter" });
    await settle();
    expect(engine.calls.map((c) => [c.name, c.input])).toEqual([
      ["keyframe.move", { item: "itm_v", keys: [{ prop: "opacity", t: sec(2) }], to_at: sec(2.5) }],
    ]);
  });

  it("switches the interpolation and deletes the selected keys", async () => {
    mount();
    const keys = [{ prop: "transform.position", t: sec(1) }];
    act(() => selectKeys("itm_v", keys));
    fireEvent.click(screen.getByRole("button", { name: "Hold" }));
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Bezier" }));
    await settle();
    fireEvent.click(screen.getByRole("button", { name: "Delete keyframe" }));
    await settle();
    expect(engine.calls.map((c) => [c.name, c.input])).toEqual([
      ["keyframe.set_interp", { item: "itm_v", keys, easing: "hold" }],
      [
        "keyframe.set_interp",
        {
          item: "itm_v",
          keys,
          interp: { kind: "bezier", x1: 0.333333, y1: 0.333333, x2: 0.666667, y2: 0.666667 },
        },
      ],
      ["keyframe.delete", { item: "itm_v", keys }],
    ]);
  });

  it("nudges with the arrow keys on the canvas: one frame, one value step", async () => {
    const { container } = mount();
    act(() => selectKeys("itm_v", [{ prop: "opacity", t: sec(2) }]));
    const canvas = container.querySelector("canvas") as HTMLCanvasElement;
    fireEvent.keyDown(canvas, { key: "ArrowLeft" });
    await settle();
    fireEvent.keyDown(canvas, { key: "ArrowDown", shiftKey: true });
    await settle();
    const edits = engine.calls.filter((c) => c.name.startsWith("keyframe."));
    expect(edits.map((c) => [c.name, c.input])).toEqual([
      [
        "keyframe.move",
        { item: "itm_v", keys: [{ prop: "opacity", t: sec(2) }], delta: -sec(1) / 30 },
      ],
      ["keyframe.set", { item: "itm_v", prop: "opacity", keys: [{ t: sec(2), value: 0.7 }] }],
    ]);
    expect(engine.names().filter((n) => n === "edit.commit_tx")).toHaveLength(2);
  });

  it("changes the tangent mode and the value view as session state", () => {
    const { handle } = mount();
    fireEvent.click(screen.getByRole("radio", { name: "Mirrored" }));
    expect(graphViewStore.getState().tangentMode).toBe("mirrored");
    const normalize = screen.getByRole("button", { name: "Normalize values" });
    fireEvent.click(normalize);
    expect(graphViewStore.getState().normalized).toBe(true);
    expect(normalize.getAttribute("aria-pressed")).toBe("true");
    handle.view.flush();
    // Normalized: every curve runs from 0 to 1, so the fitted value axis is a little more.
    expect(handle.view.viewport?.v0).toBeCloseTo(-0.1, 6);
    expect(handle.view.viewport?.v1).toBeCloseTo(1.1, 6);
    expect(engine.calls).toEqual([]);
  });

  it("passes axe", async () => {
    const { container } = mount();
    act(() => selectKeys("itm_v", [{ prop: "opacity", t: sec(2) }]));
    const results = await axe.run(container);
    expect(results.violations.map((v) => `${v.id}: ${v.nodes[0]?.html}`)).toEqual([]);
  });
});
