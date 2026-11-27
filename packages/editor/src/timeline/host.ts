// SPDX-License-Identifier: GPL-3.0-or-later
// The editor side of the timeline (WP-2A.10, arch §16 and §5.1): the interaction state machine of
// WP-2A.08 (`createTimelineMachine`) fed from the canvas, the keys, the window and the bin, its
// environment kept current from the stores, and its intents carried out. Every edit is an engine
// command (gestures through `gestureTx.ts`); the document changes only when the engine's
// `doc.patched` comes back, never here. View state (selection, playhead, zoom, scroll, snapping)
// goes to `viewStore`; what only the timeline draws (snap guides, marquee, the drop ghost, the
// cursor, the tool) to `ui`. Framework-free, so the flows run against a real engine in Vitest;
// `useTimelineHost` binds it to React.
//
// The host also owns what the machine leaves to it: track headers (select, the lock, mute and hide
// toggles, rename), markers and in/out (M, I, O), replace-by-Alt-drop, selecting what an insert,
// paste or duplicate created, and the toasts.
import type { ItemId, RpcClient, SelectionRef, SequenceId, TrackId } from "@cutweave/protocol";
import {
  type Cursor,
  createSnapPort,
  createTimelineMachine,
  type DropPreview,
  type DroppedMedia,
  type FrameScheduler,
  type GestureEdit,
  type HitResult,
  type MachineAction,
  type MachineCommand,
  type MachineEnv,
  type MachineEvent,
  type MachineIntent,
  type MachineState,
  machineKeyAction,
  type Rect,
  type SnapGuide,
  type SnapPort,
  type TimelineMachine,
  type TimelineMarker,
  type TimelineModel,
  type TimelinePointerEvent,
  type Tool,
} from "@cutweave/timeline";
import type { StoreApi } from "zustand";
import { createStore } from "zustand/vanilla";
import type { EngineStoreState } from "../engine/engineStore.ts";
import type { TKey } from "../i18n/index.ts";
import { insertDurationFor } from "../media/insertDuration.ts";
import type { DocStore } from "../state/docStore.ts";
import { selectActiveSequenceId, selectInOut, selectTimelineModel } from "../state/selectors.ts";
import type { ToastStore } from "../state/toastStore.ts";
import type { ViewStore } from "../state/viewStore.ts";
import { describeRefusal, FAILED_TITLE, heldByOther, type Translate } from "./errors.ts";
import { EngineQueue, GestureTransactions, toRpcError } from "./gestureTx.ts";
import { inOutArgs, markerArgs } from "./inOut.ts";
import type { LocksState } from "./locks.ts";
import { RttProbe } from "./rttProbe.ts";

/** What the host needs from the live `TimelineView`. */
export interface TimelineGeometry {
  /** The view's zoom and scroll as it applied them (clamped). */
  getViewState(): { readonly pxPerSec: number; readonly scrollX: number; readonly scrollY: number };
  hitTest(x: number, y: number): HitResult;
  itemsInRect(rect: Rect): readonly string[];
  timeToX(t: number): number;
  visibleRange(): { t0: number; t1: number };
  trackNameRect?(trackId: string): Rect | null;
  itemRect?(itemId: string): Rect | null;
  markerAt?(x: number, y: number): TimelineMarker | null;
  zoomBy?(factor: number, anchorX?: number): void;
  zoomToRange?(t0: number, t1: number): void;
  zoomToFit?(): void;
  scrollBy?(dx: number, dy?: number): void;
}

/** The lane area in canvas CSS px (auto-scroll starts near its edges). */
export interface LaneBox {
  readonly left: number;
  readonly right: number;
  readonly top: number;
  readonly bottom: number;
}

/** What only the timeline draws and shows, and the session's tool choices. */
export interface TimelineUi {
  readonly tool: Tool;
  /** The toolbar's link toggle: a click selects a clip's whole linked group. */
  readonly linkedSelection: boolean;
  readonly snapGuides: readonly SnapGuide[];
  readonly marquee: Rect | null;
  /** Where a dragged clip or card lands (the drop ghost). */
  readonly preview: DropPreview | null;
  /** An Alt-drop would replace this clip's media (`clip.replace`). */
  readonly replaceTarget: string | null;
  readonly cursor: Cursor;
  /** The machine's state (the panel exposes it as `data-state`). */
  readonly state: MachineState["mode"]["name"];
}

/** The track whose name is being edited in place, and why the last commit was refused. */
export interface RenameState {
  readonly trackId: string | null;
  readonly reason: string | null;
}

export type RenameOutcome = "renamed" | "unchanged" | "refused" | "failed";

/** The machine's cursors as CSS cursors. */
export const CSS_CURSOR: Readonly<Record<Cursor, string>> = {
  auto: "",
  default: "default",
  move: "move",
  copy: "copy",
  "trim-start": "w-resize",
  "trim-end": "e-resize",
  "ripple-start": "w-resize",
  "ripple-end": "e-resize",
  roll: "col-resize",
  slip: "ew-resize",
  slide: "move",
  razor: "crosshair",
  grab: "grab",
  grabbing: "grabbing",
  "zoom-in": "zoom-in",
  "zoom-out": "zoom-out",
  scrub: "ew-resize",
  "not-allowed": "not-allowed",
  locked: "not-allowed",
};

export interface TimelineHostDeps {
  readonly doc: StoreApi<DocStore>;
  readonly view: StoreApi<ViewStore>;
  readonly engine: StoreApi<EngineStoreState>;
  readonly toasts: StoreApi<ToastStore>;
  readonly t: Translate;
  /** Who holds which clip (`locksStore`); absent: nobody. */
  readonly locks?: StoreApi<Pick<LocksState, "byItem">>;
  /** The canvas window's animation frames (a detached Timeline has its own). */
  readonly frames?: FrameScheduler;
  /** Shared snapping (default: the wasm index; `initMath()` must have resolved). */
  readonly snap?: SnapPort;
  /** The transport's latest-wins `view.seek`; without it the view's playhead moves alone. */
  readonly seek?: (t: number) => void;
  /** Editing and Timeline preferences the machine reads (WP-2A.13 adds them). */
  readonly prefs?: () => Partial<Pick<MachineEnv, "snapThresholdPx" | "snapKinds" | "dropOnClip">>;
}

/** The labels of gesture undo steps. */
const HISTORY: Readonly<Record<GestureEdit, TKey>> = {
  move: "timeline.history.move",
  duplicate: "timeline.history.duplicate",
  trim: "timeline.history.trim",
  ripple_trim: "timeline.history.rippleTrim",
  roll: "timeline.history.roll",
  slip: "timeline.history.slip",
  slide: "timeline.history.slide",
  keyframe: "timeline.history.keyframe",
  insert: "timeline.history.insert",
};

/** The toast title when a gesture does not happen. */
const GESTURE_FAILED: Readonly<Record<GestureEdit, TKey>> = {
  move: FAILED_TITLE.move,
  duplicate: FAILED_TITLE.duplicate,
  trim: FAILED_TITLE.trim,
  ripple_trim: FAILED_TITLE.trim,
  roll: "timeline.failed.roll",
  slip: "timeline.failed.slip",
  slide: "timeline.failed.slide",
  keyframe: "timeline.failed.keyframe",
  insert: FAILED_TITLE.insert,
};

/** The toast title when a one-shot command is refused. */
const COMMAND_FAILED: Readonly<Record<string, TKey>> = {
  "clip.split": FAILED_TITLE.split,
  "item.delete": FAILED_TITLE.delete,
  "timeline.ripple_delete": FAILED_TITLE.delete,
  "item.move": FAILED_TITLE.move,
  "timeline.add_marker": "timeline.failed.marker",
  "timeline.set_in_out": "timeline.failed.inOut",
  "timeline.clear_in_out": "timeline.failed.inOut",
  "track.set": "timeline.failed.track",
  "track.add": "timeline.failed.track",
  "clip.replace": "timeline.failed.replace",
};

const NO_HIT: HitResult = {
  zone: "none",
  time: 0,
  trackId: null,
  trackIndex: -1,
  itemId: null,
  itemIndex: -1,
  keyframeIndex: -1,
  keyframeTime: null,
  toggle: null,
};

const NO_GUIDES: readonly SnapGuide[] = [];
const NO_ITEMS: readonly string[] = [];

type Call = (name: string, args: unknown) => Promise<unknown>;

const itemRef = (id: string): SelectionRef => ({ kind: "item", id: id as ItemId });
const trackRef = (id: string): SelectionRef => ({ kind: "track", id: id as TrackId });

/** The keys the host keeps for itself (not the machine's). */
export type HostKeyCommand =
  | "timeline.add_marker"
  | "timeline.mark_in"
  | "timeline.mark_out"
  | "track.rename";

export class TimelineHost {
  readonly ui: StoreApi<TimelineUi> = createStore<TimelineUi>(() => ({
    tool: "select",
    linkedSelection: true,
    snapGuides: NO_GUIDES,
    marquee: null,
    preview: null,
    replaceTarget: null,
    cursor: "default",
    state: "idle",
  }));

  readonly rename: StoreApi<RenameState> = createStore<RenameState>(() => ({
    trackId: null,
    reason: null,
  }));

  /** Update round trips of drags (UI → engine → patch → UI). */
  readonly probe: RttProbe;
  readonly #deps: TimelineHostDeps;
  readonly #queue = new EngineQueue();
  readonly #tx: GestureTransactions;
  readonly #machine: TimelineMachine;
  readonly #snap: SnapPort & { dispose?(): void };
  readonly #ownsSnap: boolean;
  readonly #unsubscribe: (() => void)[] = [];
  #geometry: TimelineGeometry | null = null;
  #canvas: HTMLElement | null = null;
  #lanes: (() => LaneBox) | null = null;
  #env: Partial<MachineEnv> = {};
  #selectionRefs: readonly SelectionRef[] | null = null;
  #selectionIds: readonly string[] = NO_ITEMS;
  /** The press whose "track is locked" toast went out (one per press). */
  #lockedPress: object | null = null;
  #disposed = false;

  constructor(deps: TimelineHostDeps) {
    this.#deps = deps;
    this.probe = new RttProbe(deps.doc);
    this.#ownsSnap = deps.snap === undefined;
    this.#snap = deps.snap ?? createSnapPort();
    this.#tx = new GestureTransactions({
      rpc: () => this.#rpc(),
      queue: this.#queue,
      label: (edit) => deps.t(HISTORY[edit]),
      answer: (event) => this.send(event),
      refused: (edit, error) => this.#notify(GESTURE_FAILED[edit], error),
      committed: (edit, output) => this.#committed(edit, output),
      probe: this.probe,
    });
    this.#env = this.#readEnv();
    this.#machine = createTimelineMachine(
      {
        hitTest: (x, y) => this.#geometry?.hitTest(x, y) ?? NO_HIT,
        itemsInRect: (rect) => this.#geometry?.itemsInRect(rect) ?? NO_ITEMS,
        lanes: () => this.#lanes?.() ?? { left: 0, right: 0, top: 0, bottom: 0 },
        snap: this.#snap,
        ...(deps.frames !== undefined && { frames: deps.frames }),
      },
      this.#env,
    );
    this.#machine.subscribe((intents) => {
      for (const intent of intents) this.#carry(intent);
    });
    const sync = (): void => this.#syncEnv();
    this.#unsubscribe.push(deps.doc.subscribe(sync), deps.view.subscribe(sync));
    if (deps.locks !== undefined) this.#unsubscribe.push(deps.locks.subscribe(sync));
    this.#unsubscribe.push(
      deps.engine.subscribe((state, previous) => {
        if (state.rpc === previous.rpc) return;
        this.#listen(state.rpc);
      }),
    );
    this.#listen(deps.engine.getState().rpc);
  }

  // ---- wiring ---------------------------------------------------------------------------------

  /** The machine's state (tests, debugging). */
  get state(): MachineState {
    return this.#machine.state;
  }

  /** Update requests sent so far (the engine traffic of drags). */
  get updatesSent(): number {
    return this.#tx.updates;
  }

  /** Connects the live view (null when it unmounts), its canvas and its lane box. */
  attach(
    geometry: TimelineGeometry | null,
    canvas: HTMLElement | null = null,
    lanes: (() => LaneBox) | null = null,
  ): void {
    this.#geometry = geometry;
    this.#canvas = canvas;
    this.#lanes = lanes;
    if (geometry !== null) this.#syncEnv();
  }

  /** Resolves when every edit started so far has finished (tests). */
  settled(): Promise<void> {
    return this.#queue.settled();
  }

  /** Runs one machine event. */
  send(event: MachineEvent): void {
    if (this.#disposed) return;
    this.#machine.send(event);
    this.ui.setState({ state: this.#machine.state.mode.name });
  }

  /** The panel unmounts: roll back any open gesture and stop listening. */
  dispose(): void {
    if (this.#disposed) return;
    this.#machine.dispose();
    this.#disposed = true;
    for (const off of this.#unsubscribe) off();
    this.#unsubscribe.length = 0;
    this.#offEngine?.();
    this.#offEngine = null;
    if (this.#ownsSnap) this.#snap.dispose?.();
  }

  #offEngine: (() => void) | null = null;

  /** The engine's `gesture.cancelled` (an idle gesture timed out) reaches the machine. */
  #listen(rpc: RpcClient | null): void {
    this.#offEngine?.();
    this.#offEngine = null;
    if (rpc === null) return;
    this.#offEngine = rpc.on("gesture.cancelled", (e) => {
      if (e.actor === "user") this.#tx.engineCancelled(e.tx_id, e.reason);
    });
  }

  // ---- environment ----------------------------------------------------------------------------

  #readEnv(): Partial<MachineEnv> {
    const { doc, view, locks, prefs } = this.#deps;
    const project = doc.getState().project;
    const v = view.getState();
    const live = this.#geometry?.getViewState();
    const ui = this.ui.getState();
    return {
      model: selectTimelineModel(project),
      sequence: selectActiveSequenceId(project) ?? "",
      pxPerSec: live?.pxPerSec ?? v.zoom ?? 100,
      scrollX: live?.scrollX ?? v.scroll.x,
      scrollY: live?.scrollY ?? v.scroll.y,
      playhead: v.playhead,
      selection: this.#itemSelection(v.selection),
      snapping: v.snapping,
      tool: ui.tool,
      linkedSelection: ui.linkedSelection,
      inOut: selectInOut(project),
      heldBy: locks?.getState().byItem ?? new Map(),
      ...prefs?.(),
    };
  }

  /** Sends the fields that changed since the machine last heard. */
  #syncEnv(): void {
    if (this.#disposed) return;
    const next = this.#readEnv();
    const changed: Record<string, unknown> = {};
    let any = false;
    for (const key of Object.keys(next) as (keyof MachineEnv)[]) {
      if (next[key] !== this.#env[key] && !sameArray(next[key], this.#env[key])) {
        changed[key] = next[key];
        any = true;
      }
    }
    this.#env = next;
    if (any) this.send({ type: "env", env: changed as Partial<MachineEnv> });
  }

  #itemSelection(refs: readonly SelectionRef[]): readonly string[] {
    if (refs !== this.#selectionRefs) {
      this.#selectionRefs = refs;
      const ids = refs.filter((r) => r.kind === "item").map((r) => r.id as string);
      this.#selectionIds = sameArray(ids, this.#selectionIds) ? this.#selectionIds : ids;
    }
    return this.#selectionIds;
  }

  #model(): TimelineModel {
    return selectTimelineModel(this.#deps.doc.getState().project);
  }

  #sequence(): SequenceId | null {
    return selectActiveSequenceId(this.#deps.doc.getState().project);
  }

  // ---- input ----------------------------------------------------------------------------------

  /** A pointer event from `TimelineView.onPointer` (the menu host has the secondary button). */
  handlePointer(e: TimelinePointerEvent): void {
    const { hit } = e;
    const header = hit.zone === "header" || hit.zone === "header-name";
    if (e.type === "dblclick") {
      if (hit.zone === "header-name" && hit.trackId !== null) this.beginRename(hit.trackId);
      return;
    }
    if (e.type === "down" && e.button === 0 && hit.trackId !== null) {
      if (hit.zone === "header-toggle" && hit.toggle !== null) {
        this.toggleTrack(hit.trackId, hit.toggle);
      } else if (header) {
        this.#deps.view.getState().setSelection([trackRef(hit.trackId)]);
      }
    }
    const mods = { shift: e.shiftKey, alt: e.altKey, ctrl: e.ctrlKey, meta: e.metaKey };
    switch (e.type) {
      case "down":
        this.send({ type: "pointer.down", x: e.x, y: e.y, button: e.button, ...mods });
        break;
      case "move":
        this.send({ type: "pointer.move", x: e.x, y: e.y, ...mods });
        this.#lockedDrag();
        break;
      case "up":
        this.send({ type: "pointer.up", x: e.x, y: e.y, button: e.button, ...mods });
        this.#lockedPress = null;
        break;
      case "leave":
        this.send({ type: "pointer.leave" });
        break;
      case "cancel":
        this.send({ type: "pointer.cancel" });
        break;
      default:
        return;
    }
    this.#applyCursor();
  }

  /**
   * A key in the timeline panel. `action` is what the keymap made of it (`undefined`: no binding
   * there, the machine's default table decides). Returns whether it was handled.
   */
  handleKey(
    e: {
      readonly key: string;
      readonly shiftKey?: boolean;
      readonly altKey?: boolean;
      readonly ctrlKey?: boolean;
      readonly metaKey?: boolean;
    },
    down: boolean,
    action?: MachineAction | null,
  ): boolean {
    const mods = {
      shift: e.shiftKey === true,
      alt: e.altKey === true,
      ctrl: e.ctrlKey === true,
      meta: e.metaKey === true,
    };
    if (down) {
      this.send({
        type: "key.down",
        key: e.key,
        ...mods,
        ...(action !== undefined && { action }),
      });
    } else {
      this.send({ type: "key.up", key: e.key, ...mods });
    }
    this.#applyCursor();
    // Only the timeline's own keys are taken (Ctrl+S and the rest go on to the app).
    return down && (action === undefined ? machineKeyAction(e.key, mods) : action) !== null;
  }

  /** A host key (markers, in/out, rename). Returns whether it did something. */
  runHostKey(command: HostKeyCommand): boolean {
    if (this.#busy()) return false;
    switch (command) {
      case "timeline.add_marker":
        this.addMarker();
        return true;
      case "timeline.mark_in":
        this.markIn();
        return true;
      case "timeline.mark_out":
        this.markOut();
        return true;
      case "track.rename":
        this.renameSelectedTrack();
        return true;
    }
  }

  /** A toolbar button or a menu item. */
  action(action: MachineAction): void {
    this.send({ type: "action", action });
    this.#applyCursor();
  }

  setTool(tool: Tool): void {
    this.action(`tool.${tool}`);
  }

  split(): void {
    this.action("split");
  }

  deleteSelection(ripple = false): void {
    this.action(ripple ? "ripple_delete" : "delete");
  }

  toggleSnapping(): void {
    this.action("toggle_snapping");
  }

  toggleLinkedSelection(): void {
    this.ui.setState((s) => ({ linkedSelection: !s.linkedSelection }));
    this.#syncEnv();
  }

  /** The window lost focus, or the panel goes away: roll back any gesture. */
  cancel(): void {
    this.send({ type: "blur" });
  }

  // ---- the bin --------------------------------------------------------------------------------

  /**
   * A bin card is dragged over canvas point `(x, y)`. With Alt over a clip it would replace that
   * clip's media (`clip.replace`); otherwise the machine places it (a gesture: the clip shows
   * where it lands). Returns whether a drop would land here (the drag's `dropEffect`).
   */
  mediaOver(x: number, y: number, media: DroppedMedia, mods: { alt?: boolean } = {}): boolean {
    const target = mods.alt === true ? this.#replaceTargetAt(x, y, media) : null;
    if (target !== null) {
      if (this.#machine.state.mode.name === "dropping") this.send({ type: "media.leave" });
      this.ui.setState({ replaceTarget: target, preview: null, snapGuides: NO_GUIDES });
      return true;
    }
    if (this.ui.getState().replaceTarget !== null) this.ui.setState({ replaceTarget: null });
    this.send({ type: "media.over", x, y, media, ...mods });
    return this.#machine.state.mode.name === "dropping";
  }

  /** The card was released over the canvas. */
  mediaDrop(x: number, y: number, media: DroppedMedia, mods: { alt?: boolean } = {}): void {
    const target = mods.alt === true ? this.#replaceTargetAt(x, y, media) : null;
    if (target !== null) {
      if (this.#machine.state.mode.name === "dropping") this.send({ type: "media.leave" });
      this.ui.setState({ replaceTarget: null });
      this.#oneShot("clip.replace", { item: target, media: media.id }, () => {
        this.#deps.view.getState().setSelection([itemRef(target)]);
      });
      return;
    }
    this.ui.setState({ replaceTarget: null });
    this.send({ type: "media.drop", x, y, media, ...mods });
  }

  /** The card left the canvas or the drag ended elsewhere. */
  mediaLeave(): void {
    if (this.ui.getState().replaceTarget !== null) this.ui.setState({ replaceTarget: null });
    this.send({ type: "media.leave" });
  }

  /** The clip under `(x, y)` whose media an Alt-drop of `media` may replace, or null. */
  #replaceTargetAt(x: number, y: number, media: DroppedMedia): string | null {
    const hit = this.#geometry?.hitTest(x, y);
    if (hit === undefined || hit.itemId === null) return null;
    if (hit.zone !== "body" && hit.zone !== "left-edge" && hit.zone !== "right-edge") return null;
    const model = this.#model();
    const item = model.items.find((i) => i.id === hit.itemId);
    const track = model.tracks.find((t) => t.id === item?.trackId);
    if (item === undefined || track?.locked === true) return null;
    // Pictures replace pictures and sound replaces sound; the engine checks the rest again.
    return (media.kind === "audio") === (item.kind === "audio") ? item.id : null;
  }

  // ---- headers, markers, in and out -----------------------------------------------------------

  /** A header toggle: lock, mute or hide the track (one undo step each). */
  toggleTrack(trackId: string, flag: "lock" | "mute" | "hide"): void {
    const track = this.#model().tracks.find((t) => t.id === trackId);
    if (track === undefined) return;
    const field = flag === "lock" ? "locked" : flag === "mute" ? "muted" : "hidden";
    this.#oneShot("track.set", { track: trackId, [field]: track[field] !== true });
  }

  /** Adds a sequence marker at `at` (default: the playhead), on its frame. */
  addMarker(at = this.#deps.view.getState().playhead): void {
    const args = markerArgs(this.#deps.doc.getState().project, at);
    if (args !== null) this.#oneShot("timeline.add_marker", args);
  }

  /** Sets the in point at `at` (default: the playhead), keeping the out point after it. */
  markIn(at = this.#deps.view.getState().playhead): void {
    const args = inOutArgs(this.#deps.doc.getState().project, "in", at);
    if (args !== null) this.#oneShot("timeline.set_in_out", args);
  }

  /** Sets the out point at `at` (default: the playhead), keeping the in point before it. */
  markOut(at = this.#deps.view.getState().playhead): void {
    const args = inOutArgs(this.#deps.doc.getState().project, "out", at);
    if (args !== null) this.#oneShot("timeline.set_in_out", args);
  }

  clearInOut(): void {
    const sequence = this.#sequence();
    if (sequence === null) return;
    this.#oneShot("timeline.clear_in_out", { sequence });
  }

  /** Adds a track of `kind` (the engine places it: video on top, audio below). */
  addTrack(kind: "video" | "audio"): void {
    const sequence = this.#sequence();
    if (sequence === null) return;
    this.#oneShot("track.add", { sequence, kind });
  }

  // ---- zoom -----------------------------------------------------------------------------------

  /** Zoom by `factor` around the playhead when it shows, else the lane's center. */
  zoomBy(factor: number): void {
    const g = this.#geometry;
    if (g === null) return;
    const { t0, t1 } = g.visibleRange();
    const playhead = this.#deps.view.getState().playhead;
    g.zoomBy?.(factor, playhead >= t0 && playhead <= t1 ? g.timeToX(playhead) : undefined);
  }

  zoomToFit(): void {
    this.#geometry?.zoomToFit?.();
  }

  /** Shows the selected clips across the lane (nothing selected: the whole timeline). */
  zoomToSelection(): void {
    const ids = new Set(this.#itemSelection(this.#deps.view.getState().selection));
    const items = this.#model().items.filter((i) => ids.has(i.id));
    if (items.length === 0) {
      this.zoomToFit();
      return;
    }
    const t0 = Math.min(...items.map((i) => i.start));
    const t1 = Math.max(...items.map((i) => i.start + i.duration));
    this.#geometry?.zoomToRange?.(t0, t1);
  }

  // ---- track rename (issue 149) ---------------------------------------------------------------

  /** Opens the in-place name input for `trackId`. A locked track can be renamed. */
  beginRename(trackId: string): void {
    if (!this.#model().tracks.some((t) => t.id === trackId)) return;
    this.rename.setState({ trackId, reason: null });
  }

  /** F2: renames the selected track, or the track of the selected clip. */
  renameSelectedTrack(): void {
    const { selection } = this.#deps.view.getState();
    const picked = selection.find((r) => r.kind === "track");
    if (picked !== undefined) {
      this.beginRename(picked.id);
      return;
    }
    const item = selection.find((r) => r.kind === "item");
    const owner =
      item === undefined ? undefined : this.#model().items.find((i) => i.id === item.id);
    if (owner !== undefined) this.beginRename(owner.trackId);
  }

  cancelRename(): void {
    this.rename.setState({ trackId: null, reason: null });
  }

  renameTarget(): { readonly trackId: string; readonly name: string } | null {
    const { trackId } = this.rename.getState();
    if (trackId === null) return null;
    const track = this.#model().tracks.find((t) => t.id === trackId);
    return track === undefined ? null : { trackId, name: track.name };
  }

  /** Enter or blur: `track.set{track, name}` with the trimmed name; an empty name is refused. */
  async commitRename(raw: string, how: "enter" | "blur"): Promise<RenameOutcome> {
    const target = this.renameTarget();
    if (target === null) return "unchanged";
    const { t, toasts } = this.#deps;
    const name = raw.trim();
    if (name === "") {
      const reason = t("timeline.rename.empty", { name: target.name });
      if (how === "enter") {
        this.rename.setState({ reason });
      } else {
        this.cancelRename();
        toasts
          .getState()
          .push({ tone: "warning", title: t("timeline.failed.rename"), message: reason });
      }
      return "refused";
    }
    this.cancelRename();
    if (name === target.name) return "unchanged";
    const rpc = this.#rpc();
    if (rpc === null) return "failed";
    try {
      await rpc.call("track.set", { track: target.trackId as TrackId, name });
      return "renamed";
    } catch (error) {
      this.#notify(FAILED_TITLE.rename, error);
      return "failed";
    }
  }

  trackNameRect(trackId: string): Rect | null {
    return this.#geometry?.trackNameRect?.(trackId) ?? null;
  }

  itemRect(itemId: string): Rect | null {
    return this.#geometry?.itemRect?.(itemId) ?? null;
  }

  /** The canvas x of time `t`, or null without a view. */
  timeToX(t: number): number | null {
    return this.#geometry?.timeToX(t) ?? null;
  }

  // ---- intents --------------------------------------------------------------------------------

  #carry(intent: MachineIntent): void {
    const view = this.#deps.view.getState();
    switch (intent.type) {
      case "begin_tx":
        this.#tx.begin(intent.gesture, intent.edit);
        return;
      case "update_tx":
        this.#tx.update(intent.gesture, this.#withStillLength(intent.command));
        return;
      case "commit_tx":
        this.#tx.commit(intent.gesture);
        return;
      case "cancel_tx":
        this.#tx.cancel(intent.gesture, intent.reason);
        return;
      case "command":
        this.#command(intent.command);
        return;
      case "set_selection":
        view.setSelection(intent.ids.map(itemRef));
        return;
      case "set_cursor":
        this.ui.setState({ cursor: intent.cursor });
        this.#applyCursor();
        return;
      case "set_tool":
        this.ui.setState({ tool: intent.tool });
        return;
      case "set_snapping":
        view.setSnapping(intent.on);
        return;
      case "seek":
        if (this.#deps.seek !== undefined) this.#deps.seek(intent.t);
        else view.setPlayhead(intent.t);
        return;
      case "scroll_to":
        view.setScroll(intent.scrollX, intent.scrollY);
        return;
      case "scroll_by":
        if (this.#geometry?.scrollBy !== undefined) this.#geometry.scrollBy(intent.dx, intent.dy);
        return;
      case "zoom_by":
        this.#geometry?.zoomBy?.(intent.factor, intent.anchorX);
        return;
      case "zoom_to_range":
        this.#geometry?.zoomToRange?.(intent.t0, intent.t1);
        return;
      case "snap_guides":
        this.ui.setState({ snapGuides: intent.guides.length === 0 ? NO_GUIDES : intent.guides });
        return;
      case "marquee":
        this.ui.setState({ marquee: intent.rect });
        return;
      case "preview":
        this.ui.setState({ preview: intent.preview });
        return;
      case "notice":
        this.#deps.toasts.getState().push({
          tone: "info",
          title: this.#deps.t("timeline.notice.partialGroup"),
          message: this.#deps.t("timeline.notice.partialGroupHelp"),
        });
        return;
    }
  }

  /** A still dropped from the bin keeps the length its card's "Duration…" chose (WP-2F.05). */
  #withStillLength(command: MachineCommand): MachineCommand {
    if (command.name !== "timeline.insert" || command.args.duration != null) return command;
    const length = insertDurationFor(this.#deps.doc.getState().project, command.args.media);
    return length === undefined
      ? command
      : { name: command.name, args: { ...command.args, duration: length } };
  }

  /** A one-shot machine command (split, delete, ripple delete, nudge): its own undo step. */
  #command(command: MachineCommand): void {
    this.#oneShot(command.name, command.args, (out) => {
      if (command.name === "item.delete" || command.name === "timeline.ripple_delete") {
        const gone = new Set<string>((out as { deleted?: string[] } | null)?.deleted ?? []);
        if (command.name === "timeline.ripple_delete") {
          for (const id of (command.args.items ?? []) as string[]) gone.add(id);
        }
        const view = this.#deps.view.getState();
        view.setSelection(view.selection.filter((r) => r.kind !== "item" || !gone.has(r.id)));
      }
    });
  }

  /** Runs one command after the panel's earlier engine work; a refusal is a toast. */
  #oneShot(name: string, args: unknown, done?: (output: unknown) => void): void {
    void this.#queue.run(async () => {
      const rpc = this.#rpc();
      if (rpc === null) return;
      try {
        const out = await (rpc.call.bind(rpc) as unknown as Call)(name, args);
        done?.(out);
      } catch (error) {
        this.#notify(COMMAND_FAILED[name] ?? FAILED_TITLE.move, error);
      }
    });
  }

  /** What a gesture created becomes the selection (INV-343): an insert's clips, a duplicate's copies. */
  #committed(edit: GestureEdit, output: unknown): void {
    let created: readonly string[] = [];
    if (edit === "insert") created = (output as { clips?: string[] } | null)?.clips ?? [];
    else if (edit === "duplicate") {
      created = ((output as { copies?: { copy: string }[] } | null)?.copies ?? []).map(
        (c) => c.copy,
      );
    }
    if (created.length > 0) this.#deps.view.getState().setSelection(created.map(itemRef));
  }

  // ---- feedback -------------------------------------------------------------------------------

  /** The machine's cursor overrides the view's own (except `auto`). */
  #applyCursor(): void {
    const canvas = this.#canvas;
    const cursor = this.ui.getState().cursor;
    if (canvas === null || cursor === "auto") return;
    canvas.style.cursor = CSS_CURSOR[cursor];
  }

  /** A clip on a locked track was dragged: tell the person once per press. */
  #lockedDrag(): void {
    const mode = this.#machine.state.mode;
    if (mode.name !== "pressed" || !mode.dragged) return;
    const target = mode.target;
    if (target.kind !== "none" || this.#lockedPress === target) return;
    const model = this.#model();
    const item = model.items.find((i) => i.id === target.item);
    const track = model.tracks.find((t) => t.id === item?.trackId);
    if (track?.locked !== true) return;
    this.#lockedPress = target;
    this.#deps.toasts.getState().push({
      tone: "warning",
      title: this.#deps.t(FAILED_TITLE.move),
      message: `${this.#deps.t("timeline.reason.locked")} (locked)`,
    });
  }

  #busy(): boolean {
    const name = this.#machine.state.mode.name;
    return name !== "idle" && name !== "hover";
  }

  #rpc(): RpcClient | null {
    return this.#deps.engine.getState().rpc;
  }

  #notify(title: TKey, error: unknown): void {
    const { t, toasts } = this.#deps;
    const refusal = toRpcError(error);
    toasts.getState().push({
      // A clip an agent or a plugin holds: agent blue, like its "in use by" mark (WP-2A.12).
      tone: heldByOther(refusal) ? "agent" : "warning",
      title: t(title),
      message: withCode(describeRefusal(refusal, t), refusal.code),
    });
  }
}

/** The machine code after the reason, unless the reason already names it. */
export function withCode(message: string, code: string): string {
  return message.includes(`(${code})`) || /\([a-z_]+\)$/.test(message)
    ? message
    : `${message} (${code})`;
}

function sameArray(a: unknown, b: unknown): boolean {
  if (!Array.isArray(a) || !Array.isArray(b) || a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}
