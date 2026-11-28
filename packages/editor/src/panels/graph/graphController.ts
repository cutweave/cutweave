// SPDX-License-Identifier: GPL-3.0-or-later
// The graph editor's controller (WP-2C.07), framework-free. While the panel is open it listens
// to the document, the selection, the playhead and the key selection, gives the canvas its
// curves and turns the canvas's events into engine commands; while the panel is closed it does
// not exist, so nothing of the graph editor runs on a pointer move, a drag update or a patch.
//
// It talks to the canvas (`GraphView`) directly, never through React: a document patch redraws
// the canvas and renders nothing. The panel's shell (toolbar, curve list) reads `graphShellStore`,
// which changes only when what the shell shows changed.
import type { FrameRate, Interp, RpcClient } from "@cutweave/protocol";
import { type SnapIndex, snap as snapMath, time as timeMath } from "@cutweave/protocol/wasm";
import { FLICKS_PER_SECOND } from "@cutweave/timeline";
import type {
  GraphAction,
  GraphEdit,
  GraphEditorHandle,
  GraphOptions,
  GraphPointRef,
  GraphSnap,
  GraphView,
} from "@cutweave/ui";
import type { StoreApi } from "zustand/vanilla";
import { type EngineStoreState, engineStore } from "../../engine/engineStore.ts";
import { PROP_LABELS } from "../../inspector/labels.ts";
import { menuDialogStore } from "../../menus/dialogs.ts";
import { clipLabel } from "../../menus/surfaces/facts.ts";
import { preferences } from "../../prefs/prefsStore.ts";
import { type DocStore, docStore } from "../../state/docStore.ts";
import { type PlaybackStore, playbackStore } from "../../state/playbackStore.ts";
import { selectActiveSequenceId } from "../../state/selectors.ts";
import { type ToastStore, toastStore } from "../../state/toastStore.ts";
import { type ViewState, viewStore } from "../../state/viewStore.ts";
import { describeRefusal, type Translate } from "../../timeline/errors.ts";
import { type FrameScheduler, toRpcError } from "../../timeline/gesture.ts";
import {
  addKey,
  clearKeys,
  copyKeys,
  deleteKeys,
  type KeyAddress,
  type KeySelection,
  keyClipboardStore,
  keySelectionStore,
  pasteKeys,
  pruneKeys,
  sameKey,
  selectKeys,
  setEasing,
} from "../../timeline/keyframes.ts";
import {
  type EditOrigin,
  type MoveOutput,
  moveStep,
  nudgeStep,
  STRAIGHT_BEZIER,
  tangentStep,
  valueStep as valueFieldStep,
} from "./graphEdits.ts";
import { GraphGesture, type GraphStep } from "./graphGesture.ts";
import {
  type BuiltCurves,
  buildCurves,
  CompiledCache,
  type EditorCurve,
  keyOfPoint,
  NO_CURVES,
  pointsOf,
  sameClips,
  selectedClips,
  valueStep,
} from "./graphModel.ts";
import {
  type GraphShellState,
  type GraphViewState,
  graphShellStore,
  graphViewStore,
  graphWork,
  initialGraphShell,
  NO_READOUT,
  type ShellProp,
  type ShellReadout,
} from "./graphStore.ts";

export interface GraphControllerDeps {
  readonly doc: StoreApi<DocStore>;
  readonly view: StoreApi<ViewState>;
  readonly playback: StoreApi<PlaybackStore>;
  readonly engine: StoreApi<EngineStoreState>;
  readonly toasts: StoreApi<ToastStore>;
  readonly keys: StoreApi<KeySelection>;
  readonly graph: StoreApi<GraphViewState>;
  readonly shell: StoreApi<GraphShellState>;
  readonly t: Translate;
  /** Moves the playhead (the time ruler). */
  seek(t: number): void;
  /** Opens the easing dialog of a key. */
  openEasing(item: string, prop: string, t: number): void;
  readonly frames?: FrameScheduler;
}

export function defaultGraphDeps(t: Translate, seek: (t: number) => void): GraphControllerDeps {
  return {
    doc: docStore,
    view: viewStore,
    playback: playbackStore,
    engine: engineStore,
    toasts: toastStore,
    keys: keySelectionStore,
    graph: graphViewStore,
    shell: graphShellStore,
    t,
    seek,
    openEasing: (item, prop, at) =>
      menuDialogStore.getState().open({ kind: "keyframe-curve", item, prop, t: at }),
  };
}

type FailedTitle =
  | "graph.failed.move"
  | "graph.failed.handle"
  | "graph.failed.value"
  | "graph.failed.time"
  | "graph.failed.easing"
  | "timeline.failed.keyframeDelete"
  | "timeline.failed.keyframeAdd"
  | "timeline.failed.keyframeCopy"
  | "timeline.failed.keyframePaste";

const DEFAULT_RATE: FrameRate = { num: 30, den: 1 };
/** The most keys given to the snap index. */
const MAX_SNAP_KEYS = 5000;

const selectedItems = (view: ViewState): readonly string[] => {
  const out: string[] = [];
  for (const ref of view.selection) if (ref.kind === "item") out.push(ref.id);
  return out;
};

const sameList = (a: readonly string[], b: readonly string[]): boolean =>
  a.length === b.length && a.every((x, i) => x === b[i]);

const sameProps = (a: readonly ShellProp[], b: readonly ShellProp[]): boolean =>
  a.length === b.length &&
  a.every((p, i) => {
    const q = b[i] as ShellProp;
    return (
      p.id === q.id &&
      p.label === q.label &&
      p.hidden === q.hidden &&
      sameList(p.colors.map(String), q.colors.map(String))
    );
  });

const sameReadout = (a: ShellReadout, b: ShellReadout): boolean =>
  a.count === b.count &&
  a.time === b.time &&
  a.value === b.value &&
  a.kind === b.kind &&
  a.editable === b.editable &&
  a.holdOnly === b.holdOnly;

/** A key a menu was asked for: its clip, property and sequence time, and where to open. */
export interface MenuKey {
  readonly item: string;
  readonly prop: string;
  readonly at: number;
  /** Where the menu opens when it was not asked for at the pointer. */
  readonly point: { readonly x: number; readonly y: number } | null;
}

export class GraphController {
  readonly #deps: GraphControllerDeps;
  readonly #cache = new CompiledCache();
  #handle: GraphEditorHandle | null = null;
  #view: GraphView | null = null;
  #off: (() => void)[] = [];
  #built: BuiltCurves = NO_CURVES;
  #items: readonly string[] = [];
  #hidden: readonly string[] = [];
  #locks = "";
  #rate: FrameRate = DEFAULT_RATE;
  #frame = Math.round(FLICKS_PER_SECOND / 30);
  #gesture: { session: GraphGesture; origin: EditOrigin; move: boolean } | null = null;
  #snapIndex: SnapIndex | null = null;
  /** Engine work in order: an edit starts after the one before it ended. */
  #queue: Promise<unknown> = Promise.resolve();

  constructor(deps: GraphControllerDeps) {
    this.#deps = deps;
  }

  /** The curves the canvas shows now. */
  get curves(): readonly EditorCurve[] {
    return this.#built.curves;
  }

  /** Resolves when every edit started so far has finished (tests). */
  settled(): Promise<void> {
    return this.#queue.then(
      () => undefined,
      () => undefined,
    );
  }

  // ---- lifecycle ----------------------------------------------------------------------------

  /** The canvas is there: start listening and show the selection's curves. */
  attach(handle: GraphEditorHandle): void {
    this.detach();
    const d = this.#deps;
    this.#handle = handle;
    const view = handle.view;
    this.#view = view;
    view.events = {
      select: (points, mode) => this.#select(points, mode),
      gesture: (phase, edit) => this.#onGesture(phase, edit),
      add: (curve, t) => this.#insert(curve, t),
      nudge: (frames, steps) => void this.nudge(frames, steps),
      action: (name) => void this.action(name),
      scrub: (t) => d.seek(this.#toFrame(Math.max(0, t))),
      viewport: (viewport) => {
        if (d.graph.getState().viewport !== viewport) {
          d.graph.setState({ viewport, shown: this.#items.join(",") });
        }
      },
    };
    const graph = d.graph.getState();
    view.setNormalized(graph.normalized);
    view.setTangentMode(graph.tangentMode);
    this.#built = NO_CURVES;
    this.#refresh(true);
    // The viewport kept from last time belongs to the clips it showed: other clips are fitted.
    view.setViewport(graph.shown === this.#items.join(",") ? graph.viewport : null);
    view.setPlayhead(d.playback.getState().playhead);

    this.#off.push(
      d.doc.subscribe((s, prev) => {
        if (s.project !== prev.project) this.#refresh(false);
      }),
      d.view.subscribe((s, prev) => {
        if (s.selection !== prev.selection) this.#selectionChanged();
      }),
      d.playback.subscribe((s, prev) => {
        if (s.playhead !== prev.playhead) this.#view?.setPlayhead(s.playhead);
      }),
      d.keys.subscribe(() => this.#keysChanged()),
      d.graph.subscribe((s, prev) => this.#graphChanged(s, prev)),
    );
  }

  /** The canvas is going away: stop listening, roll back a drag in progress, free the math. */
  detach(): void {
    for (const off of this.#off) off();
    this.#off = [];
    const gesture = this.#gesture;
    if (gesture !== null) {
      this.#gesture = null;
      void gesture.session.finish(false);
      if (this.#deps.keys.getState().offset !== 0) this.#deps.keys.setState({ offset: 0 });
    }
    this.#freeSnap();
    if (this.#view !== null) this.#view.events = {};
    this.#view = null;
    this.#handle = null;
    this.#built = NO_CURVES;
    this.#cache.dispose();
    // Nothing is shown any more: the next opening starts from what it finds then.
    if (this.#deps.shell.getState() !== initialGraphShell) {
      this.#deps.shell.setState(initialGraphShell, true);
    }
  }

  // ---- the document and the selection -------------------------------------------------------

  /**
   * Looks at the selected clips again. Only when one of them is a different object (it changed,
   * or the selection did) are the curves rebuilt; any other patch costs one lookup per selected
   * clip and draws nothing.
   */
  #refresh(force: boolean): void {
    const d = this.#deps;
    const view = this.#view;
    if (view === null) return;
    const project = d.doc.getState().project;
    const items = selectedItems(d.view.getState());
    const hidden = d.graph.getState().hidden;
    const sequence = selectActiveSequenceId(project);
    const found = selectedClips(project, sequence, items);
    const locks = found.map((f) => (f.locked ? "1" : "0")).join("");
    this.#items = items;
    const rate =
      (sequence === null ? undefined : project?.sequences[sequence]?.format.frame_rate) ??
      DEFAULT_RATE;
    if (rate.num !== this.#rate.num || rate.den !== this.#rate.den || force) {
      this.#rate = rate;
      this.#frame = timeMath.frameToTime(1, rate);
      view.setOptions(this.#options());
    }
    if (
      !force &&
      hidden === this.#hidden &&
      locks === this.#locks &&
      sameClips(this.#built, found)
    ) {
      return;
    }
    graphWork.rebuilds += 1;
    this.#hidden = hidden;
    this.#locks = locks;
    this.#built = buildCurves(
      {
        project,
        sequence,
        items,
        hidden,
        label: (prop) => {
          const key = PROP_LABELS[prop];
          return key === undefined ? prop : d.t(key);
        },
        clipLabel,
      },
      this.#cache,
    );
    view.setCurves(this.#built.curves);
    if (this.#gesture === null) pruneKeys(project);
    this.#syncSelection();
    this.#publishShell();
  }

  #selectionChanged(): void {
    const d = this.#deps;
    const items = selectedItems(d.view.getState());
    if (sameList(items, this.#items)) return;
    // Other clips: their curves, fitted.
    const graph = d.graph.getState();
    if (graph.viewport !== null || graph.mask !== null)
      d.graph.setState({ viewport: null, mask: null });
    this.#refresh(false);
    this.#view?.setViewport(null);
  }

  #keysChanged(): void {
    this.#syncSelection();
    this.#publishShell();
  }

  #graphChanged(s: GraphViewState, prev: GraphViewState): void {
    const view = this.#view;
    if (view === null) return;
    if (s.tangentMode !== prev.tangentMode) view.setTangentMode(s.tangentMode);
    if (s.normalized !== prev.normalized) {
      view.setNormalized(s.normalized);
      // The value axis means something else now: fit it again.
      view.setViewport(null);
      if (s.viewport !== null) this.#deps.graph.setState({ viewport: null });
    }
    if (s.hidden !== prev.hidden) this.#refresh(false);
    if (s.mask !== prev.mask) this.#keysChanged();
    if (s.viewport !== prev.viewport && s.viewport !== view.viewport) view.setViewport(s.viewport);
  }

  #syncSelection(): void {
    const d = this.#deps;
    this.#view?.setSelection(
      pointsOf(this.#built.curves, d.keys.getState(), d.graph.getState().mask),
    );
  }

  /** The shell's curve list and the selected key in numbers; written only when they changed. */
  #publishShell(): void {
    const d = this.#deps;
    const props: ShellProp[] = this.#built.groups.map((g) => ({
      id: `${g.item}|${g.prop}`,
      prop: g.prop,
      label: g.label,
      colors: g.colors,
      hidden: g.hidden,
    }));
    const readout = this.#readout();
    const now = d.shell.getState();
    const nextProps = sameProps(now.props, props) ? now.props : props;
    const nextReadout = sameReadout(now.readout, readout) ? now.readout : readout;
    if (nextProps !== now.props || nextReadout !== now.readout) {
      d.shell.setState({ props: nextProps, readout: nextReadout });
    }
  }

  #readout(): ShellReadout {
    const d = this.#deps;
    const selection = d.keys.getState();
    if (selection.item === null || selection.keys.length === 0) return NO_READOUT;
    const mask = d.graph.getState().mask;
    const points = pointsOf(this.#built.curves, selection, mask);
    if (points.length === 0) return NO_READOUT;
    const byId = new Map(this.#built.curves.map((c) => [c.id, c]));
    let time: number | null | undefined;
    let value: number | null | undefined;
    let kind: ShellReadout["kind"] | undefined;
    let editable = true;
    let holdOnly = false;
    for (const p of points) {
      const curve = byId.get(p.curve) as EditorCurve;
      const t = curve.times[p.index] as number;
      const v = curve.values[p.index] as number;
      const k = (curve.interps[p.index]?.kind ?? null) as ShellReadout["kind"];
      time = time === undefined || time === t ? t : null;
      value = value === undefined || value === v ? v : null;
      kind = kind === undefined || kind === k ? k : null;
      if (curve.locked === true) editable = false;
      if (curve.shape === "bool") holdOnly = true;
    }
    return {
      count: selection.keys.length,
      time: selection.keys.length === 1 ? (time ?? null) : null,
      value: value ?? null,
      kind: kind ?? null,
      editable,
      holdOnly,
    };
  }

  // ---- the canvas's options -----------------------------------------------------------------

  #options(): GraphOptions {
    const rate = this.#rate;
    return {
      second: FLICKS_PER_SECOND,
      frame: this.#frame,
      formatTime: (t) => timeMath.timecode(Math.max(0, Math.round(t)), rate),
      formatValue: (v) => String(Number(v.toPrecision(6))),
      snap: (t, unitsPerPx) => this.#snap(t, unitsPerPx),
    };
  }

  #toFrame(t: number): number {
    return timeMath.snapToFrame(Math.max(0, Math.round(t)), this.#rate, "nearest");
  }

  /**
   * A dragged key's time: on a frame always, and on the playhead, a clip's edge or another key
   * when snapping is on and one is within the snap distance (the timeline's own snapping, in
   * the shared wasm module).
   */
  #snap(t: number, unitsPerPx: number): GraphSnap {
    const d = this.#deps;
    const at = Math.max(0, Math.round(t));
    if (!d.view.getState().snapping) return { t: this.#toFrame(at), guide: null };
    this.#snapIndex ??= this.#buildSnapIndex();
    const result = this.#snapIndex.snap(at, [0], {
      pxPerSecond: FLICKS_PER_SECOND / unitsPerPx,
      thresholdPx: preferences.get("timeline.snap_distance"),
    });
    return { t: result.t, guide: result.target?.t ?? null };
  }

  #buildSnapIndex(): SnapIndex {
    const d = this.#deps;
    const selection = d.keys.getState();
    const moving = new Set(
      selection.keys.map((k) => `${selection.item}|${k.prop}|${k.t + selection.offset}`),
    );
    const candidates: { kind: "playhead" | "edge" | "marker"; t: number }[] = [
      { kind: "playhead", t: d.playback.getState().playhead },
    ];
    const seen = new Set<string>();
    for (const curve of this.#built.curves) {
      if (!seen.has(curve.item)) {
        seen.add(curve.item);
        candidates.push(
          { kind: "edge", t: curve.start },
          { kind: "edge", t: curve.start + curve.duration },
        );
      }
      if (curve.comp !== 0) continue;
      for (let i = 0; i < curve.itemTimes.length && candidates.length < MAX_SNAP_KEYS; i++) {
        if (moving.has(`${curve.item}|${curve.prop}|${curve.itemTimes[i]}`)) continue;
        candidates.push({ kind: "marker", t: curve.times[i] as number });
      }
    }
    return snapMath.createIndex(
      this.#rate,
      candidates.filter((c) => c.t >= 0),
    );
  }

  #freeSnap(): void {
    this.#snapIndex?.free();
    this.#snapIndex = null;
  }

  // ---- engine access ------------------------------------------------------------------------

  #rpc(): RpcClient | null {
    const { rpc, blocked } = this.#deps.engine.getState();
    return blocked === null ? rpc : null;
  }

  #notify(title: FailedTitle, error: unknown): void {
    const d = this.#deps;
    d.toasts.getState().push({
      tone: "danger",
      title: d.t(title),
      message: describeRefusal(toRpcError(error), d.t),
    });
  }

  /** One engine call after the panel's earlier work; a refusal is a toast. */
  #run(
    title: FailedTitle,
    work: (call: (name: string, args?: unknown) => Promise<unknown>) => Promise<void>,
  ): Promise<void> {
    const run = async (): Promise<void> => {
      const rpc = this.#rpc();
      if (rpc === null) return;
      const call = rpc.call.bind(rpc) as unknown as (
        name: string,
        args?: unknown,
      ) => Promise<unknown>;
      try {
        await work(call);
      } catch (error) {
        this.#notify(title, error);
        // A typed number that was refused: the fields show the stored value again.
        this.#deps.shell.setState((s) => ({ epoch: s.epoch + 1 }));
      }
    };
    const done = this.#queue.then(run, run);
    this.#queue = done;
    return done;
  }

  /** One step outside a drag (an arrow key, a typed number): a gesture of one update. */
  #once(
    label: string,
    title: FailedTitle,
    step: GraphStep | null,
    origin: EditOrigin,
  ): Promise<void> {
    const rpc = this.#rpc();
    if (rpc === null || step === null) return Promise.resolve();
    const session = new GraphGesture({
      rpc,
      label,
      after: this.#queue,
      ...(this.#deps.frames && { frames: this.#deps.frames }),
      onAbort: (error) => this.#notify(title, error),
    });
    session.update(step);
    const done = session.finish(true).then((outcome) => {
      if (outcome.kind === "committed") {
        const moved = (outcome.output as MoveOutput | null)?.keys;
        if (moved !== undefined) selectKeys(origin.item, moved);
      } else if (outcome.kind === "refused") {
        this.#notify(title, outcome.error);
        this.#deps.shell.setState((s) => ({ epoch: s.epoch + 1 }));
      }
    });
    this.#queue = this.#queue.then(() => done);
    return done;
  }

  #origin(): EditOrigin | null {
    const selection = this.#deps.keys.getState();
    if (selection.item === null || selection.keys.length === 0) return null;
    return { item: selection.item, keys: selection.keys, curves: this.#built.curves };
  }

  #masked = (curve: EditorCurve): boolean => {
    const mask = this.#deps.graph.getState().mask;
    return mask === null || mask.includes(`${curve.prop}|${curve.comp}`);
  };

  // ---- the canvas's events ------------------------------------------------------------------

  #select(points: readonly GraphPointRef[], mode: "replace" | "toggle" | "add"): void {
    const d = this.#deps;
    const curves = this.#built.curves;
    const found = points.flatMap((p) => {
      const k = keyOfPoint(curves, p);
      return k === null ? [] : [k];
    });
    const current = d.keys.getState();
    if (found.length === 0) {
      if (mode === "replace") {
        if (d.graph.getState().mask !== null) d.graph.setState({ mask: null });
        clearKeys();
        this.#announce();
      }
      return;
    }
    // Keys of one clip at a time: the clip already selected from, else the first one hit.
    const item =
      mode !== "replace" &&
      current.item !== null &&
      found.some((f) => f.curve.item === current.item)
        ? current.item
        : (found[0] as (typeof found)[number]).curve.item;
    const own = found.filter((f) => f.curve.item === item);
    const unique = (keys: readonly KeyAddress[]): KeyAddress[] =>
      keys.filter((k, i) => keys.findIndex((x) => sameKey(x, k)) === i);
    const comps = [...new Set(own.map((f) => `${f.curve.prop}|${f.curve.comp}`))];
    const keys = unique(own.map((f) => f.key));
    const mask = d.graph.getState().mask;
    if (mode === "replace" || current.item !== item) {
      d.graph.setState({ mask: comps });
      selectKeys(item, keys);
    } else if (mode === "add") {
      if (mask !== null) d.graph.setState({ mask: [...new Set([...mask, ...comps])] });
      selectKeys(item, unique([...current.keys, ...keys]));
    } else {
      const kept = current.keys.filter((k) => !keys.some((x) => sameKey(x, k)));
      const added = keys.filter((k) => !current.keys.some((x) => sameKey(x, k)));
      if (mask !== null && added.length > 0) {
        d.graph.setState({ mask: [...new Set([...mask, ...comps])] });
      }
      selectKeys(item, [...kept, ...added]);
    }
    this.#announce();
  }

  /** Says the selection to assistive technology. */
  #announce(): void {
    const d = this.#deps;
    const handle = this.#handle;
    if (handle === null) return;
    const selection = d.keys.getState();
    const count = selection.keys.length;
    if (count === 0) {
      handle.announce(d.t("graph.announce.none"));
      return;
    }
    if (count > 1) {
      handle.announce(d.t("graph.announce.several", { count: String(count) }));
      return;
    }
    const key = selection.keys[0] as KeyAddress;
    const curve = this.#built.curves.find(
      (c) => c.item === selection.item && c.prop === key.prop && this.#masked(c),
    );
    const index = curve?.itemTimes.indexOf(key.t) ?? -1;
    if (curve === undefined || index < 0) return;
    handle.announce(
      d.t("graph.announce.one", {
        name: curve.label,
        index: String(index + 1),
        count: String(curve.itemTimes.length),
        time: timeMath.timecode(Math.max(0, curve.times[index] as number), this.#rate),
        value: String(Number((curve.values[index] as number).toPrecision(6))),
      }),
    );
  }

  #onGesture(phase: "update" | "commit" | "cancel", edit: GraphEdit | null): void {
    const d = this.#deps;
    if (phase === "update" && edit !== null) {
      let gesture = this.#gesture;
      if (gesture === null) {
        const rpc = this.#rpc();
        const origin = this.#origin();
        if (rpc === null || origin === null) return;
        const move = edit.type === "move";
        const title: FailedTitle = move ? "graph.failed.move" : "graph.failed.handle";
        const session: GraphGesture = new GraphGesture({
          rpc,
          label: d.t(move ? "timeline.history.keyframe" : "graph.history.handle"),
          after: this.#queue,
          ...(d.frames && { frames: d.frames }),
          onApplied: (output) => {
            // The keys are somewhere else in the document now: they stay selected there.
            const shift = (output as MoveOutput | null)?.shift ?? 0;
            if (this.#gesture?.session === session && d.keys.getState().offset !== shift) {
              d.keys.setState({ offset: shift });
            }
          },
          onAbort: (error) => {
            if (this.#gesture?.session === session) this.#endGesture();
            this.#view?.cancelDrag();
            this.#notify(title, error);
          },
        });
        gesture = { session, origin, move };
        this.#gesture = gesture;
      }
      const step =
        edit.type === "move" ? moveStep(gesture.origin, edit) : tangentStep(gesture.origin, edit);
      if (step !== null) gesture.session.update(step);
      return;
    }
    const gesture = this.#gesture;
    if (gesture === null) return;
    const title: FailedTitle = gesture.move ? "graph.failed.move" : "graph.failed.handle";
    const done = gesture.session.finish(phase === "commit").then((outcome) => {
      if (this.#gesture === gesture) this.#endGesture();
      if (outcome.kind === "committed") {
        const moved = (outcome.output as MoveOutput | null)?.keys;
        if (moved !== undefined) selectKeys(gesture.origin.item, moved);
      } else if (outcome.kind === "refused" || (outcome.kind === "aborted" && !outcome.reported)) {
        this.#notify(title, outcome.error);
      }
      pruneKeys(d.doc.getState().project);
      this.#keysChanged();
    });
    this.#queue = this.#queue.then(() => done);
  }

  #endGesture(): void {
    this.#gesture = null;
    this.#freeSnap();
    const keys = this.#deps.keys;
    if (keys.getState().offset !== 0) keys.setState({ offset: 0 });
  }

  #insert(curveId: string, t: number): void {
    const curve = this.#built.curves.find((c) => c.id === curveId);
    if (curve === undefined) return;
    const at = this.#toFrame(t);
    this.#deps.graph.setState({ mask: null });
    void this.#run("timeline.failed.keyframeAdd", (call) =>
      addKey(call, curve.item, curve.prop, at),
    );
  }

  // ---- what the toolbar and the keys do -----------------------------------------------------

  /** The arrow keys: by frames in time, or by steps in value. One undo step. */
  nudge(frames: number, steps: number): Promise<void> {
    const origin = this.#origin();
    if (origin === null) return Promise.resolve();
    const step = nudgeStep(
      origin,
      frames * this.#frame,
      (curve) => steps * valueStep(curve.prop),
      this.#masked,
    );
    const d = this.#deps;
    return this.#once(
      d.t(frames !== 0 ? "timeline.history.keyframe" : "graph.history.value"),
      frames !== 0 ? "graph.failed.move" : "graph.failed.value",
      step,
      origin,
    );
  }

  /** The Value field: every selected point takes the value. */
  setValue(value: number): Promise<void> {
    const origin = this.#origin();
    if (origin === null || !Number.isFinite(value)) return Promise.resolve();
    return this.#once(
      this.#deps.t("graph.history.value"),
      "graph.failed.value",
      valueFieldStep(origin, value, this.#masked),
      origin,
    );
  }

  /** The Time field: the one selected key goes to the typed time. `false`: not a time. */
  setTime(text: string): boolean {
    const origin = this.#origin();
    const parsed = timeMath.parse(text, this.#rate);
    if (origin === null || origin.keys.length !== 1 || !parsed.ok) return false;
    const key = origin.keys[0] as KeyAddress;
    const to = parsed.flicks;
    void this.#run("graph.failed.time", async (call) => {
      const out = (await call("keyframe.move", {
        item: origin.item,
        keys: [{ prop: key.prop, t: key.t }],
        to_at: to,
      })) as { moved?: readonly { prop: string; t: number }[] } | null;
      const moved = (out?.moved ?? []).map((m) => ({ prop: m.prop, t: m.t }));
      if (moved.length > 0) selectKeys(origin.item, moved);
    });
    return true;
  }

  /** The time the Time field shows for `t` (a timecode at the sequence's rate). */
  formatTime(t: number): string {
    return timeMath.timecode(Math.max(0, Math.round(t)), this.#rate);
  }

  /** The interpolation switch: how the segments starting at the selected keys move. */
  setKind(kind: "hold" | "linear" | "bezier" | "spring"): Promise<void> {
    const origin = this.#origin();
    if (origin === null) return Promise.resolve();
    const how: { readonly easing: "hold" | "linear" | "spring" } | { readonly interp: Interp } =
      kind === "bezier" ? { interp: STRAIGHT_BEZIER } : { easing: kind };
    return this.#run("graph.failed.easing", (call) =>
      setEasing(call, origin.item, origin.keys, how),
    );
  }

  /** The easing dialog (presets, numbers) of the first selected key. */
  openEasing(): void {
    const origin = this.#origin();
    const key = origin?.keys[0];
    if (origin === null || key === undefined) return;
    this.#deps.openEasing(origin.item, key.prop, key.t);
  }

  fit(): void {
    this.#view?.fit(false);
  }

  zoom(factor: number): void {
    this.#view?.zoomBy(factor, "both");
  }

  /** The key under a point of the window and what its menu acts on, or null. */
  keyAt(clientX: number, clientY: number): MenuKey | null {
    const ref = this.#view?.keyAtClient(clientX, clientY) ?? null;
    const target = ref === null ? null : this.#menuTarget(ref);
    return target === null ? null : { ...target, point: null };
  }

  /** The first selected key and where it is drawn, for the menu key. */
  selectedKey(): MenuKey | null {
    const hit = this.#view?.selectedAtClient() ?? null;
    const target = hit === null ? null : this.#menuTarget(hit.ref);
    return hit === null || target === null
      ? null
      : { ...target, point: { x: hit.clientX, y: hit.clientY } };
  }

  #menuTarget(ref: GraphPointRef): { item: string; prop: string; at: number } | null {
    const found = keyOfPoint(this.#built.curves, ref);
    if (found === null) return null;
    const selection = this.#deps.keys.getState();
    const selected =
      selection.item === found.curve.item && selection.keys.some((k) => sameKey(k, found.key));
    if (!selected) this.#select([ref], "replace");
    return {
      item: found.curve.item,
      prop: found.curve.prop,
      at: found.curve.start + found.key.t,
    };
  }

  action(name: GraphAction): Promise<void> {
    const d = this.#deps;
    const selection = d.keys.getState();
    switch (name) {
      case "clear":
        this.#select([], "replace");
        return Promise.resolve();
      case "easing":
        this.openEasing();
        return Promise.resolve();
      case "delete": {
        const { item, keys } = selection;
        if (item === null || keys.length === 0) return Promise.resolve();
        return this.#run("timeline.failed.keyframeDelete", (call) => deleteKeys(call, item, keys));
      }
      case "copy": {
        const { item, keys } = selection;
        if (item === null || keys.length === 0) return Promise.resolve();
        return this.#run("timeline.failed.keyframeCopy", async (call) => {
          await copyKeys(call, item, keys);
        });
      }
      case "paste": {
        const payload = keyClipboardStore.getState().payload;
        const item = selection.item ?? this.#built.curves[0]?.item ?? this.#items[0];
        if (payload === null || item === undefined) return Promise.resolve();
        const at = d.playback.getState().playhead;
        return this.#run("timeline.failed.keyframePaste", (call) =>
          pasteKeys(call, payload, item, at),
        );
      }
      case "selectAll": {
        const item = selection.item ?? this.#built.curves[0]?.item;
        if (item === undefined) return Promise.resolve();
        const keys: KeyAddress[] = [];
        for (const curve of this.#built.curves) {
          if (curve.item !== item || curve.comp !== 0) continue;
          for (const t of curve.itemTimes) keys.push({ prop: curve.prop, t });
        }
        d.graph.setState({ mask: null });
        selectKeys(item, keys);
        this.#announce();
        return Promise.resolve();
      }
      case "previousKey":
      case "nextKey":
        this.#step(name === "nextKey" ? 1 : -1);
        return Promise.resolve();
    }
  }

  /** Alt+Left and Alt+Right: the key before or after the selected one on its curve. */
  #step(direction: 1 | -1): void {
    const d = this.#deps;
    const curves = this.#built.curves;
    if (curves.length === 0) return;
    const selection = d.keys.getState();
    const key = selection.keys[selection.keys.length - 1];
    let curve = curves.find(
      (c) => c.item === selection.item && c.prop === key?.prop && this.#masked(c),
    );
    let index: number;
    if (curve === undefined || key === undefined) {
      // Nothing selected: the first key at or after the playhead (or the last before it).
      curve = curves[0] as EditorCurve;
      const playhead = d.playback.getState().playhead;
      const after = Array.prototype.findIndex.call(curve.times, (t: number) => t >= playhead);
      index =
        direction === 1
          ? after < 0
            ? curve.times.length - 1
            : after
          : Math.max(0, (after < 0 ? curve.times.length : after) - 1);
    } else {
      const now = curve.itemTimes.indexOf(key.t);
      index = Math.min(curve.itemTimes.length - 1, Math.max(0, now + direction));
    }
    this.#select([{ curve: curve.id, index }], "replace");
  }
}
