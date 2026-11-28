// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2C.07 against a REAL headless engine: the graph editor's controller and canvas model on the
// engine's live document. The curve the canvas draws is the engine's curve (`keyframe.eval`,
// within 1e-6); a handle drag stores the bezier numbers it should and is one undo step; keys
// dragged in time and value are one gesture transaction whose every update replaces the one
// before; a refused position falls back to the last accepted one. Only the canvas drawing is
// stubbed. Skips with a message when the engine cannot be built.
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Interp, RpcClient, SequenceId, TrackId } from "@cutweave/protocol";
import {
  type EngineHandle,
  resolveEngineBinary,
  startEngine,
} from "@cutweave/protocol/test-support/engine";
import { mathModuleBytes } from "@cutweave/protocol/test-support/math-wasm";
import { initMath } from "@cutweave/protocol/wasm";
import { type GraphEdit, type GraphTheme, GraphView } from "@cutweave/ui";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { createEngineStore } from "../../engine/engineStore.ts";
import { type EngineRuntime, startEngineRuntime } from "../../engine/runtime.ts";
import { i18n } from "../../i18n/index.ts";
import { createDocStore } from "../../state/docStore.ts";
import { createJobStore } from "../../state/jobStore.ts";
import { createPlaybackStore } from "../../state/playbackStore.ts";
import { createToastStore } from "../../state/toastStore.ts";
import { createViewStore } from "../../state/viewStore.ts";
import { translator } from "../../timeline/errors.ts";
import { EMPTY_KEY_SELECTION, keySelectionStore, selectKeys } from "../../timeline/keyframes.ts";
import { GraphController } from "./graphController.ts";
import { curveId, type StoredGraphKey } from "./graphModel.ts";
import {
  graphShellStore,
  graphViewStore,
  initialGraphShell,
  initialGraphView,
} from "./graphStore.ts";

const F = 705_600_000;
const sec = (s: number) => Math.round(s * F);
const FRAME = F / 30;

let skipReason: string | undefined;
let handle: EngineHandle | undefined;
let runtime: EngineRuntime;
let rpc: RpcClient;
let mediaDir: string;
const stores = {
  doc: createDocStore(),
  view: createViewStore(),
  playback: createPlaybackStore(),
  jobs: createJobStore(),
  engine: createEngineStore(),
};
const toasts = createToastStore();
let controller: GraphController;
let view: GraphView;
const frames: (() => void)[] = [];
/** Runs the frames the gesture is waiting for. */
const frame = (): void => {
  for (const cb of frames.splice(0)) cb();
};

const THEME: GraphTheme = {
  surface: "",
  ruler: "",
  grid: "",
  gridStrong: "",
  text: "",
  textStrong: "",
  playhead: "",
  selected: "",
  onSelected: "",
  marquee: "",
  guide: "",
  curves: ["a", "b", "c"],
  font: "",
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

const noopContext = new Proxy(
  {},
  {
    get: (_t, prop) => (prop === "measureText" ? () => ({ width: 0 }) : () => undefined),
    set: () => true,
  },
) as CanvasRenderingContext2D;

/** A canvas that draws nothing: the view's model and math are real. */
function fakeCanvas(): HTMLCanvasElement {
  return {
    width: 0,
    height: 0,
    style: {},
    getContext: () => noopContext,
    setAttribute: () => undefined,
    focus: () => undefined,
    getBoundingClientRect: () => ({ left: 0, top: 0, width: 864, height: 424 }),
  } as unknown as HTMLCanvasElement;
}

beforeAll(async () => {
  const binary = resolveEngineBinary();
  if (binary.unavailable !== undefined) {
    skipReason = binary.unavailable;
    console.warn(`graph editor engine tests skipped: ${skipReason}`);
    return;
  }
  await initMath(mathModuleBytes());
  mediaDir = mkdtempSync(join(tmpdir(), "cutweave-graph-e2e-"));
  // Generated here, never committed: the stub backend probes by extension (10 s, video + audio).
  writeFileSync(join(mediaDir, "clip.mp4"), Buffer.alloc(4096, 7));
  handle = await startEngine(binary.path, ["--dev-stub-media", "--allow-root", mediaDir]);
  const info = handle.info;
  runtime = startEngineRuntime(
    { getConnection: () => Promise.resolve({ ...info, port: handle?.port ?? 0 }) },
    stores,
    { view: { debounceMs: 50 } },
  );
  rpc = runtime.rpc;
  await runtime.connection.whenReady();
  await until(() => stores.doc.getState().status === "ready");

  view = new GraphView(
    fakeCanvas(),
    { second: F, frame: FRAME, formatTime: String, formatValue: String },
    { theme: THEME, frames: { request: () => 1, cancel: () => undefined } },
  );
  view.resize(864, 424, 1);
  controller = new GraphController({
    doc: stores.doc,
    view: stores.view,
    playback: stores.playback,
    engine: stores.engine,
    toasts,
    keys: keySelectionStore,
    graph: graphViewStore,
    shell: graphShellStore,
    t: translator(i18n.t.bind(i18n)),
    seek: () => undefined,
    openEasing: () => undefined,
    frames: { request: (cb) => frames.push(cb), cancel: () => frames.splice(0) },
  });
}, 20 * 60_000);

afterAll(async () => {
  controller?.detach();
  await runtime?.stop();
  await handle?.stop();
  if (mediaDir !== undefined) rmSync(mediaDir, { recursive: true, force: true });
});

async function until(condition: () => boolean, timeoutMs = 10_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!condition()) {
    if (Date.now() > deadline) throw new Error("timed out waiting for the engine and the stores");
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}

const settledDoc = () => stores.doc.getState().rev === stores.doc.getState().committedRev;
const history = async () => (await rpc.call("edit.history", { limit: 500 })).entries;
const openTransactions = async () => (await rpc.call("edit.locks", {})).transactions.length;

interface Seeded {
  sequence: SequenceId;
  video: TrackId;
  media: string;
}
let project: Seeded | undefined;
let clip = "";

const EASE = "ease_in_out";

/** The clip's stored keys of a property, from the engine's document. */
function keysOf(prop: "transform.position" | "opacity"): StoredGraphKey[] {
  const tracks = Object.values(stores.doc.getState().project?.sequences ?? {})[0]?.tracks ?? {};
  for (const track of Object.values(tracks)) {
    const item = track.items[clip as never] as
      | { visual?: { transform?: { position?: unknown }; opacity?: unknown } }
      | undefined;
    if (item === undefined) continue;
    const animatable = prop === "opacity" ? item.visual?.opacity : item.visual?.transform?.position;
    return [...((animatable as { keyframes?: StoredGraphKey[] } | undefined)?.keyframes ?? [])];
  }
  return [];
}

/**
 * A 4 s clip at 1 s with a position animation of three eased keys (item times 0, 1 and 2 s) and
 * two opacity keys, selected, with the graph editor open on it.
 */
async function animated(): Promise<void> {
  if (project === undefined) {
    const created = await rpc.call("project.new", { name: "Graph" });
    const imported = await rpc.call("media.import", { paths: [join(mediaDir, "clip.mp4")] });
    project = {
      sequence: created.sequence,
      video: created.video_track,
      media: imported.media_ids[0] as string,
    };
    await rpc.call("track.set_magnetic", { track: created.video_track, magnetic: false });
  }
  controller.detach();
  const items = Object.values(stores.doc.getState().project?.sequences ?? {}).flatMap((s) =>
    Object.values(s.tracks).flatMap((t) => Object.keys(t.items)),
  );
  if (items.length > 0) await rpc.call("item.delete", { items: items as never });
  const inserted = await rpc.call("timeline.insert", {
    sequence: project.sequence,
    track: project.video,
    media: project.media as never,
    at: "1s",
    duration: "4s",
    link: false,
  });
  clip = inserted.clips[0] as string;
  await rpc.call("keyframe.set", {
    item: clip as never,
    prop: "transform.position",
    keys: [
      { t: 0, value: { x: -400, y: 0 }, easing: EASE },
      { t: sec(1), value: { x: 200, y: -150 }, easing: EASE },
      { t: sec(2), value: { x: 0, y: 300 } },
    ] as never,
  });
  await rpc.call("keyframe.set", {
    item: clip as never,
    prop: "opacity",
    keys: [
      { t: 0, value: 0 },
      { t: sec(2), value: 0.8 },
    ] as never,
  });
  await until(() => keysOf("transform.position").length === 3 && keysOf("opacity").length === 2);
  await until(settledDoc);
  stores.view.getState().setSelection([{ kind: "item", id: clip as never }]);
  controller.attach({ view, announce: () => undefined });
}

const X = () => curveId(clip, "transform.position", 0);
const Y = () => curveId(clip, "transform.position", 1);
const MIDDLE = { prop: "transform.position", t: sec(1) };

/** One drag: every edit as an update with a frame in between, then the release. */
async function drag(edits: readonly GraphEdit[], release: "commit" | "cancel" = "commit") {
  for (const edit of edits) {
    view.events.gesture?.("update", edit);
    frame();
    // The engine answers and the document follows before the next pointer position.
    await new Promise((resolve) => setTimeout(resolve, 30));
  }
  view.events.gesture?.(release, edits[edits.length - 1] ?? null);
  await controller.settled();
  // A rolled-back gesture leaves the live revision ahead of the committed one: nothing to wait for.
  if (release === "commit") await until(settledDoc);
}

/** The engine's own samples of a property over the clip (`keyframe.eval`). */
async function engineSamples(
  prop: "transform.position" | "opacity",
  from: number,
  to: number,
  n: number,
) {
  const out = await rpc.call("keyframe.eval", {
    item: clip as never,
    prop,
    from,
    to,
    samples: n,
  });
  return out.samples.map((s) => s.value);
}

/** What the canvas draws: the curve's sampler over the same times. */
function drawnSamples(id: string, from: number, to: number, n: number): number[] {
  const curve = controller.curves.find((c) => c.id === id);
  if (curve === undefined) throw new Error(`no curve ${id}`);
  const out = new Float64Array(n);
  curve.sample(from, to, n, out);
  return [...out];
}

async function expectDrawnIsEngine(): Promise<void> {
  // Sequence times from half a second before the first key to half a second after the last.
  const from = sec(0.5);
  const to = sec(3.5);
  const n = 301;
  const position = (await engineSamples("transform.position", from, to, n)) as {
    x: number;
    y: number;
  }[];
  const x = drawnSamples(X(), from, to, n);
  const y = drawnSamples(Y(), from, to, n);
  let worst = 0;
  for (let i = 0; i < n; i++) {
    worst = Math.max(
      worst,
      Math.abs((x[i] as number) - (position[i]?.x as number)),
      Math.abs((y[i] as number) - (position[i]?.y as number)),
    );
  }
  expect(worst).toBeLessThan(1e-6);
  const opacity = (await engineSamples("opacity", from, to, n)) as number[];
  const drawn = drawnSamples(curveId(clip, "opacity", 0), from, to, n);
  for (let i = 0; i < n; i++)
    expect(Math.abs((drawn[i] as number) - (opacity[i] as number))).toBeLessThan(1e-6);
}

beforeEach(() => {
  if (skipReason !== undefined) return;
  keySelectionStore.setState(EMPTY_KEY_SELECTION, true);
  graphViewStore.setState(initialGraphView, true);
  graphShellStore.setState(initialGraphShell, true);
  toasts.getState().clear();
  frames.splice(0);
});

describe("the graph editor against the real engine (WP-2C.07)", () => {
  it("draws the engine's curve: the samples equal keyframe.eval within 1e-6", async (ctx) => {
    if (skipReason !== undefined) return ctx.skip();
    await animated();
    expect(controller.curves.map((c) => c.id)).toEqual([X(), Y(), curveId(clip, "opacity", 0)]);
    await expectDrawnIsEngine();
  });

  it("stores a dragged handle's bezier numbers, redraws the engine's curve, and undoes in one step", async (ctx) => {
    if (skipReason !== undefined) return ctx.skip();
    await animated();
    const original = keysOf("transform.position");
    expect(original[1]?.interp).toMatchObject({ kind: "bezier", x1: 0.42, preset: EASE });
    const before = (await history()).length;
    selectKeys(clip, [MIDDLE]);

    // The out handle of the middle key dragged, through three positions, to (0.25, 0.5).
    const at = (x: number, y: number): GraphEdit => ({
      type: "tangent",
      curve: X(),
      index: 1,
      side: "out",
      x,
      y,
      opposite: null,
    });
    await drag([at(0.4, 0.1), at(0.3, 0.3), at(0.25, 0.5)]);

    const stored = keysOf("transform.position")[1]?.interp as Extract<Interp, { kind: "bezier" }>;
    expect(stored.kind).toBe("bezier");
    expect(stored.x1).toBeCloseTo(0.25, 3);
    expect(stored.y1).toBeCloseTo(0.5, 3);
    // The handle arriving at the next key is untouched; the preset hint is gone with its shape.
    expect(stored.x2).toBeCloseTo(0.58, 6);
    expect(stored.y2).toBeCloseTo(1, 6);
    expect(stored.preset ?? null).toBeNull();
    // The segment before the key was not touched (free tangents).
    expect(keysOf("transform.position")[0]).toEqual(original[0]);
    await expectDrawnIsEngine();

    // One undo step, named.
    const entries = await history();
    expect(entries.length - before).toBe(1);
    expect(entries[0]?.label).toBe("Adjust curve handle");
    expect(await openTransactions()).toBe(0);
    await rpc.call("edit.undo", {});
    await until(() => (keysOf("transform.position")[1]?.interp as { x1?: number }).x1 === 0.42);
    expect(keysOf("transform.position")).toEqual(original);
    await expectDrawnIsEngine();
  });

  it("moves the handle across the key too when tangents are tied", async (ctx) => {
    if (skipReason !== undefined) return ctx.skip();
    await animated();
    selectKeys(clip, [MIDDLE]);
    const before = (await history()).length;
    await drag([
      {
        type: "tangent",
        curve: X(),
        index: 1,
        side: "out",
        x: 0.25,
        y: 0.5,
        opposite: { x: 0.75, y: 1.1 },
      },
    ]);
    const keys = keysOf("transform.position");
    expect(keys[1]?.interp).toMatchObject({ kind: "bezier", x1: 0.25, y1: 0.5, x2: 0.58, y2: 1 });
    expect(keys[0]?.interp).toMatchObject({ kind: "bezier", x1: 0.42, y1: 0, x2: 0.75, y2: 1.1 });
    // Two commands, one undo step.
    expect((await history()).length - before).toBe(1);
    await expectDrawnIsEngine();
  });

  it("drags keys in time and value as one gesture: each update replaces the last", async (ctx) => {
    if (skipReason !== undefined) return ctx.skip();
    await animated();
    const original = keysOf("transform.position");
    const before = (await history()).length;
    selectKeys(clip, [MIDDLE]);
    const move = (frames: number, dv: number): GraphEdit => ({
      type: "move",
      grabbed: { curve: X(), index: 1 },
      dt: frames * FRAME,
      dv: dv === 0 ? {} : { [X()]: dv },
    });
    // Later and up, further, then back a little: the keys are addressed as they were at first.
    await drag([move(3, 20), move(9, 80), move(6, 50)]);

    const keys = keysOf("transform.position");
    expect(keys.map((k) => k.t)).toEqual([0, sec(1) + 6 * FRAME, sec(2)]);
    // X moved by 50; Y, whose curve was not dragged, kept its value; the easing stayed.
    expect(keys[1]?.value).toEqual({ x: 250, y: -150 });
    expect(keys[1]?.interp).toEqual(original[1]?.interp);
    // The key is still selected, where it is now.
    expect(keySelectionStore.getState()).toMatchObject({
      item: clip,
      keys: [{ prop: "transform.position", t: sec(1) + 6 * FRAME }],
      offset: 0,
    });
    const entries = await history();
    expect(entries.length - before).toBe(1);
    expect(entries[0]?.label).toBe("Move keyframe");
    expect(await openTransactions()).toBe(0);
    await expectDrawnIsEngine();

    await rpc.call("edit.undo", {});
    await until(() => keysOf("transform.position")[1]?.t === sec(1));
    expect(keysOf("transform.position")).toEqual(original);
  });

  it("keeps the last accepted position when the pointer ends on another key", async (ctx) => {
    if (skipReason !== undefined) return ctx.skip();
    await animated();
    selectKeys(clip, [MIDDLE]);
    const move = (frames: number): GraphEdit => ({
      type: "move",
      grabbed: { curve: X(), index: 1 },
      dt: frames * FRAME,
      dv: { [X()]: 10 },
    });
    // 30 frames later is the last key's own time: the engine refuses that position.
    await drag([move(12), move(30)]);
    const keys = keysOf("transform.position");
    expect(keys.map((k) => k.t)).toEqual([0, sec(1) + 12 * FRAME, sec(2)]);
    expect(keys[1]?.value).toEqual({ x: 210, y: -150 });
    expect(await openTransactions()).toBe(0);
  });

  it("rolls a cancelled drag back and leaves no undo step", async (ctx) => {
    if (skipReason !== undefined) return ctx.skip();
    await animated();
    const original = keysOf("transform.position");
    const before = (await history()).length;
    selectKeys(clip, [MIDDLE]);
    await drag(
      [{ type: "move", grabbed: { curve: Y(), index: 1 }, dt: 5 * FRAME, dv: { [Y()]: 40 } }],
      "cancel",
    );
    await until(() => keysOf("transform.position")[1]?.t === sec(1));
    expect(keysOf("transform.position")).toEqual(original);
    expect((await history()).length).toBe(before);
    expect(await openTransactions()).toBe(0);
    expect(keySelectionStore.getState().offset).toBe(0);
  });

  it("nudges, sets a typed value and time, switches the interpolation: one undo step each", async (ctx) => {
    if (skipReason !== undefined) return ctx.skip();
    await animated();
    const before = (await history()).length;
    selectKeys(clip, [{ prop: "opacity", t: sec(2) }]);
    await controller.nudge(-2, 0);
    await until(() => keysOf("opacity")[1]?.t === sec(2) - 2 * FRAME);
    await controller.nudge(0, -10);
    await until(() => Math.abs((keysOf("opacity")[1]?.value as number) - 0.7) < 1e-9);
    await controller.setValue(5); // beyond the property's range: stops at 1
    await until(() => keysOf("opacity")[1]?.value === 1);
    expect(controller.setTime("00:00:03:15")).toBe(true);
    await controller.settled();
    await until(() => keysOf("opacity")[1]?.t === sec(2.5));
    await controller.setKind("bezier");
    await until(() => keysOf("opacity")[1]?.interp.kind === "bezier");
    await until(settledDoc);
    expect((await history()).length - before).toBe(5);
    expect(keySelectionStore.getState().keys).toEqual([{ prop: "opacity", t: sec(2.5) }]);
    expect(toasts.getState().notices).toEqual([]);
    await expectDrawnIsEngine();
  });

  it("tells the person, without blocking, when the engine refuses an edit", async (ctx) => {
    if (skipReason !== undefined) return ctx.skip();
    await animated();
    await rpc.call("track.set", { track: (project as Seeded).video, locked: true });
    await until(settledDoc);
    selectKeys(clip, [{ prop: "opacity", t: sec(2) }]);
    await controller.action("delete");
    await until(() => toasts.getState().notices.length === 1);
    expect(toasts.getState().notices[0]).toMatchObject({
      tone: "danger",
      title: "Keyframe not deleted",
    });
    expect(keysOf("opacity")).toHaveLength(2);
    await rpc.call("track.set", { track: (project as Seeded).video, locked: false });
  });
});
