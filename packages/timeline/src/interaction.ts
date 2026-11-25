// SPDX-License-Identifier: GPL-3.0-or-later
// The timeline's interaction state machine (WP-1M.06; arch §16): a pure reducer
// `reduce(state, event) -> { state, intents }`, XState-lite and framework-free. It turns pointer
// and key input (with `TimelineView`'s hit-test results) into **intents**: what the user asked
// for, in timeline terms. The editor turns intents into engine commands and view-store updates;
// nothing here talks to the engine or touches the DOM, so every transition is unit-testable.
//
//   Idle ──move──▶ Hovering ──down──▶ Selecting       (empty lane: click deselects, drag = marquee)
//                                   ├▶ Moving          (clip body)
//                                   ├▶ TrimmingStart   (left edge)
//                                   ├▶ TrimmingEnd     (right edge)
//                                   ├▶ ScrubbingRuler  (ruler, playhead flag)
//                                   └▶ Panning         (middle button)
//   every gesture state ──up──▶ Hovering; ──Esc / pointer-cancel──▶ Idle (rolled back)
//   media-drag-over ──▶ DroppingMedia (a bin card dragged over the lanes; WP-1M.03) ──media-drop──▶ Idle
//      with an `insert` intent; Esc or `media-drag-leave` ──▶ Idle without one.
//
// Gestures (move, trim) follow arch §5.2: once the pointer travels `DRAG_THRESHOLD_PX` from the
// press, `begin-gesture` opens an ephemeral transaction; each change of the snapped position
// emits one `move` / `trim` update (absolute: start and track, or edge time); release emits
// `commit-gesture`, Esc `cancel-gesture`. A press that never travels is a click: it selects and
// opens no transaction. Times in intents are always frame-aligned (see `snap.ts`).
import type { HitResult, HitZone } from "./hit-test.ts";
import type { Rect, TimelineItem, TimelineModel, TrackKind } from "./model.ts";
import { SNAP_PX, snapSpan, snapTargets, snapTolerance } from "./snap.ts";
import { FLICKS_PER_SECOND, frameDurationFlicks, snapToFrame } from "./time.ts";
import type { TimelinePointerEvent } from "./timeline-view.ts";

/** Pointer travel (CSS px) that turns a press into a drag. */
export const DRAG_THRESHOLD_PX = 3;

/** What the machine reads from the host: the model and the view state it is acting on. */
export interface InteractionEnv {
  readonly model: TimelineModel;
  /** Zoom, CSS pixels per second (pixel distances to flicks). */
  readonly pxPerSec: number;
  /** Scroll position (panning starts from it). */
  readonly scrollX: number;
  readonly scrollY: number;
  /** Playhead, flicks (a snap target; split point). */
  readonly playhead: number;
  /** Selected item ids, in selection order. */
  readonly selection: readonly string[];
  /** Snapping to playhead and clip edges (`N` toggles it). The frame grid always applies. */
  readonly snapping: boolean;
}

export const DEFAULT_ENV: InteractionEnv = {
  model: { fps: { num: 24, den: 1 }, tracks: [], items: [] },
  pxPerSec: 100,
  scrollX: 0,
  scrollY: 0,
  playhead: 0,
  selection: [],
  snapping: true,
};

// ---- states ---------------------------------------------------------------------------------

export type InteractionStateName = Mode["name"];

export interface IdleMode {
  readonly name: "idle";
}

export interface HoveringMode {
  readonly name: "hovering";
  readonly zone: HitZone;
  readonly itemId: string | null;
}

export interface SelectingMode {
  readonly name: "selecting";
  readonly originX: number;
  readonly originY: number;
  /** Shift / Ctrl / Cmd: add to the selection instead of replacing it. */
  readonly additive: boolean;
  /** The marquee once the pointer has travelled; null while it is still a click. */
  readonly marquee: Rect | null;
}

/** What moving and trimming share: a press on a clip that may become a gesture. */
interface DragBase {
  readonly item: string;
  readonly originX: number;
  readonly originY: number;
  /** Timeline time under the press, flicks. */
  readonly originTime: number;
  /** Past the threshold: the gesture is open. */
  readonly dragging: boolean;
  /** Alt was held at some point of the drag: edit only this item (`unlink: true`). */
  readonly unlink: boolean;
  /** Snap targets, captured when the drag starts (excludes the item and its linked group). */
  readonly targets: readonly number[];
  /** The target the last update snapped to (drawn as a guide). */
  readonly guide: number | null;
}

export interface MovingMode extends DragBase {
  readonly name: "moving";
  readonly trackKind: TrackKind;
  /** Start and duration when pressed. */
  readonly start0: number;
  readonly duration: number;
  /** Track and start of the last update (the press position before the first). */
  readonly toTrack: string;
  readonly at: number;
  /** Clicked inside a multi-selection: a click without travel selects only this item. */
  readonly collapse: boolean;
}

export interface TrimmingMode extends DragBase {
  readonly name: "trimming-start" | "trimming-end";
  /** The edge's time when pressed. */
  readonly edge0: number;
  /** Frame-aligned limits of the edge: media bounds, neighbours, at least one frame long. */
  readonly min: number;
  readonly max: number;
  /** Edge time of the last update. */
  readonly at: number;
}

export interface ScrubbingMode {
  readonly name: "scrubbing-ruler";
  /** Playhead before the scrub (Esc restores it). */
  readonly origin: number;
  readonly at: number;
  readonly targets: readonly number[];
}

export interface PanningMode {
  readonly name: "panning";
  readonly originX: number;
  readonly originY: number;
  readonly scrollX0: number;
  readonly scrollY0: number;
}

/** What a media card carries while it is dragged over the timeline (WP-1M.03). */
export interface MediaDrag {
  readonly id: string;
  readonly kind: "video" | "audio" | "image";
  /** Flicks; `null` for stills (they drop with `STILL_DROP_DURATION`). */
  readonly duration: number | null;
}

/** Length a still image gets when dropped (the engine's default for `timeline.insert`). */
export const STILL_DROP_DURATION = 5 * FLICKS_PER_SECOND;

/** A media card is over the lanes: where it would land if dropped now. */
export interface DroppingMediaMode {
  readonly name: "dropping-media";
  readonly media: MediaDrag;
  /** Snapped, frame-aligned start of the clip. */
  readonly at: number;
  /** The track it would go on (the one under the pointer when it fits, else the first that does). */
  readonly trackId: string;
  readonly targets: readonly number[];
  readonly guide: number | null;
}

export type Mode =
  | IdleMode
  | HoveringMode
  | SelectingMode
  | MovingMode
  | TrimmingMode
  | ScrubbingMode
  | PanningMode
  | DroppingMediaMode;

export interface InteractionState {
  readonly mode: Mode;
  readonly env: InteractionEnv;
}

export function initialState(env: Partial<InteractionEnv> = {}): InteractionState {
  return { mode: IDLE, env: { ...DEFAULT_ENV, ...env } };
}

// ---- events ---------------------------------------------------------------------------------

export interface PointerInput {
  /** Canvas-relative CSS pixels. */
  readonly x: number;
  readonly y: number;
  readonly hit: HitResult;
  /** 0 primary, 1 middle, 2 secondary. */
  readonly button: number;
  readonly shiftKey: boolean;
  readonly altKey: boolean;
  readonly ctrlKey: boolean;
  readonly metaKey: boolean;
}

export interface MediaDragPoint {
  readonly x: number;
  readonly y: number;
  readonly hit: HitResult;
}

export interface KeyInput {
  /** `KeyboardEvent.key`. */
  readonly key: string;
  readonly shiftKey?: boolean;
  readonly altKey?: boolean;
  readonly ctrlKey?: boolean;
  readonly metaKey?: boolean;
}

export type InteractionEvent =
  | ({ readonly type: "pointer-down" } & PointerInput)
  | ({ readonly type: "pointer-move" } & PointerInput)
  | ({ readonly type: "pointer-up" } & PointerInput)
  | { readonly type: "pointer-leave" }
  /** A media card is dragged over the canvas (`x`, `y` are canvas-relative, `hit` its hit-test). */
  | ({ readonly type: "media-drag-over"; readonly media: MediaDrag } & MediaDragPoint)
  /** The card was released over the canvas. */
  | ({ readonly type: "media-drop"; readonly media: MediaDrag } & MediaDragPoint)
  /** The card left the canvas (or the drag ended elsewhere). */
  | { readonly type: "media-drag-leave" }
  /** The pointer was taken away (capture lost, window blurred): like Esc. */
  | { readonly type: "pointer-cancel" }
  | ({ readonly type: "key" } & KeyInput)
  /** Toolbar "Split": split the selection (or the clip under the playhead) at the playhead. */
  | { readonly type: "split" }
  /** Delete the selected items. */
  | { readonly type: "delete" }
  | { readonly type: "toggle-snapping" }
  /** The host ended the gesture itself (the engine refused it and it was rolled back). */
  | { readonly type: "gesture-aborted" }
  /** The host's model or view state changed. */
  | { readonly type: "env"; readonly env: Partial<InteractionEnv> };

/** Maps a `TimelineView` pointer event to a machine event (`null` for ones it ignores). */
export function fromPointerEvent(e: TimelinePointerEvent): InteractionEvent | null {
  const input: PointerInput = {
    x: e.x,
    y: e.y,
    hit: e.hit,
    button: e.button,
    shiftKey: e.shiftKey,
    altKey: e.altKey,
    ctrlKey: e.ctrlKey,
    metaKey: e.metaKey,
  };
  switch (e.type) {
    case "down":
      return { type: "pointer-down", ...input };
    case "move":
      return { type: "pointer-move", ...input };
    case "up":
      return { type: "pointer-up", ...input };
    case "leave":
      return { type: "pointer-leave" };
    case "cancel":
      return { type: "pointer-cancel" };
    default:
      return null;
  }
}

// ---- intents --------------------------------------------------------------------------------

export type Intent =
  /** Replace the selection with `ids` (empty: deselect all). */
  | { readonly kind: "select"; readonly ids: readonly string[] }
  /** Select the items inside the marquee (the host knows the geometry). */
  | { readonly kind: "select-rect"; readonly rect: Rect; readonly additive: boolean }
  /** Draw (or clear) the marquee. */
  | { readonly kind: "marquee"; readonly rect: Rect | null }
  /** Open a gesture transaction for `item` (`edit.begin_tx{kind:"gesture"}`). */
  | { readonly kind: "begin-gesture"; readonly gesture: "move" | "trim"; readonly item: string }
  /** Gesture update: `item.move{item, start: at, to_track, unlink}`. */
  | {
      readonly kind: "move";
      readonly item: string;
      readonly toTrack: string;
      readonly at: number;
      readonly unlink: boolean;
    }
  /** Gesture update: `clip.trim{item, edge, to: at, unlink}`. */
  | {
      readonly kind: "trim";
      readonly item: string;
      readonly edge: "start" | "end";
      readonly at: number;
      readonly unlink: boolean;
    }
  /** Release: `edit.commit_tx` (one undo step). */
  | { readonly kind: "commit-gesture" }
  /** Esc / lost pointer: `edit.cancel_tx` (everything rolls back). */
  | { readonly kind: "cancel-gesture" }
  /** Snap guide lines to draw (empty: none). */
  | { readonly kind: "snap-guides"; readonly times: readonly number[] }
  /** A media card was dropped: `timeline.insert{media, track: trackId, at, overlap: "new_track"}`. */
  | {
      readonly kind: "insert";
      readonly media: string;
      readonly trackId: string;
      readonly at: number;
    }
  /** `clip.split{item, at}` for each item (one undo step). */
  | { readonly kind: "split"; readonly items: readonly string[]; readonly at: number }
  /** `item.delete{items}`. */
  | { readonly kind: "delete"; readonly items: readonly string[] }
  /** Move the playhead (frame-aligned). */
  | { readonly kind: "seek"; readonly at: number }
  /** Scroll the view (panning). */
  | { readonly kind: "scroll"; readonly scrollX: number; readonly scrollY: number }
  /** F2: rename the selected track (the host knows which, and owns the input). */
  | { readonly kind: "rename-track" }
  /** Snapping was switched on or off. */
  | { readonly kind: "set-snapping"; readonly on: boolean };

export interface Transition {
  readonly state: InteractionState;
  readonly intents: readonly Intent[];
}

// ---- reducer --------------------------------------------------------------------------------

const IDLE: IdleMode = { name: "idle" };
const NONE: readonly Intent[] = [];

/** The pure transition function. Returns the same `state` object when nothing changed. */
export function reduce(state: InteractionState, event: InteractionEvent): Transition {
  switch (event.type) {
    case "env":
      return { state: { ...state, env: { ...state.env, ...event.env } }, intents: NONE };
    case "pointer-down":
      return onDown(state, event);
    case "pointer-move":
      return onMove(state, event);
    case "pointer-up":
      return onUp(state, event);
    case "pointer-leave":
      return state.mode.name === "hovering" ? to(state, IDLE) : same(state);
    case "media-drag-over":
      return onMediaOver(state, event);
    case "media-drop":
      return onMediaDrop(state, event);
    case "media-drag-leave":
      return state.mode.name === "dropping-media" ? leaveDrop(state) : same(state);
    case "pointer-cancel":
      return cancel(state);
    case "gesture-aborted":
      return aborted(state);
    case "key":
      return onKey(state, event);
    case "split":
      return split(state);
    case "delete":
      return remove(state);
    case "toggle-snapping":
      return toggleSnapping(state);
  }
}

/** Whether the machine is in the middle of a gesture (keys like `S` and `Delete` wait). */
export function isBusy(state: InteractionState): boolean {
  return state.mode.name !== "idle" && state.mode.name !== "hovering";
}

/** Whether a move or trim gesture is open (a transaction exists for it). */
export function isGestureOpen(state: InteractionState): boolean {
  const m = state.mode;
  return (
    (m.name === "moving" || m.name === "trimming-start" || m.name === "trimming-end") && m.dragging
  );
}

function same(state: InteractionState): Transition {
  return { state, intents: NONE };
}

function to(state: InteractionState, mode: Mode, intents: readonly Intent[] = NONE): Transition {
  return { state: { ...state, mode }, intents };
}

function hovering(e: PointerInput): HoveringMode {
  return { name: "hovering", zone: e.hit.zone, itemId: e.hit.itemId };
}

const modified = (e: { shiftKey?: boolean; ctrlKey?: boolean; metaKey?: boolean }): boolean =>
  e.shiftKey === true || e.ctrlKey === true || e.metaKey === true;

const travelled = (m: { originX: number; originY: number }, e: PointerInput): boolean =>
  Math.abs(e.x - m.originX) >= DRAG_THRESHOLD_PX || Math.abs(e.y - m.originY) >= DRAG_THRESHOLD_PX;

function findItem(model: TimelineModel, id: string): TimelineItem | undefined {
  return model.items.find((it) => it.id === id);
}

function trackKind(model: TimelineModel, trackId: string | null): TrackKind | undefined {
  return trackId === null ? undefined : model.tracks.find((t) => t.id === trackId)?.kind;
}

function select(state: InteractionState, ids: readonly string[]): InteractionState {
  return { ...state, env: { ...state.env, selection: ids } };
}

// ---- pointer down ---------------------------------------------------------------------------

function onDown(state: InteractionState, e: PointerInput): Transition {
  if (isBusy(state)) return same(state); // a second button during a gesture
  const { env } = state;
  if (e.button === 1) {
    return to(state, {
      name: "panning",
      originX: e.x,
      originY: e.y,
      scrollX0: env.scrollX,
      scrollY0: env.scrollY,
    });
  }
  // The right button is the host's: `TimelineView` reports it as a `contextmenu` event (menus
  // open on that, not on the press, so the release cannot land on the menu), never as a gesture.
  if (e.button !== 0) return same(state);

  switch (e.hit.zone) {
    case "ruler":
    case "playhead": {
      const targets = env.snapping ? snapTargets(env.model) : [];
      const at = scrubTime(env, targets, e.hit.time);
      const mode: ScrubbingMode = { name: "scrubbing-ruler", origin: env.playhead, at, targets };
      return to(state, mode, [{ kind: "seek", at }]);
    }
    case "body":
    case "keyframe":
    case "left-edge":
    case "right-edge":
      return pressItem(state, e);
    case "lane":
    case "none":
      return to(state, {
        name: "selecting",
        originX: e.x,
        originY: e.y,
        additive: modified(e),
        marquee: null,
      });
    default:
      return to(state, hovering(e)); // headers: nothing to do yet
  }
}

function pressItem(state: InteractionState, e: PointerInput): Transition {
  const { env } = state;
  const id = e.hit.itemId;
  const item = id === null ? undefined : findItem(env.model, id);
  if (id === null || item === undefined) return to(state, hovering(e));

  const inSelection = env.selection.includes(id);
  if (modified(e)) {
    // Shift / Ctrl / Cmd click toggles the item; it does not start a drag.
    const ids = inSelection ? env.selection.filter((s) => s !== id) : [...env.selection, id];
    return to(select(state, ids), hovering(e), [{ kind: "select", ids }]);
  }
  let next = state;
  const intents: Intent[] = [];
  if (!inSelection) {
    next = select(state, [id]);
    intents.push({ kind: "select", ids: [id] });
  }
  const base = {
    item: id,
    originX: e.x,
    originY: e.y,
    originTime: e.hit.time,
    dragging: false,
    unlink: false,
    targets: [] as readonly number[],
    guide: null,
  };
  if (e.hit.zone === "left-edge" || e.hit.zone === "right-edge") {
    const startEdge = e.hit.zone === "left-edge";
    const { min, max } = trimLimits(env.model, item, startEdge ? "start" : "end");
    const edge0 = startEdge ? item.start : item.start + item.duration;
    const mode: TrimmingMode = {
      ...base,
      name: startEdge ? "trimming-start" : "trimming-end",
      edge0,
      min,
      max,
      at: edge0,
    };
    return to(next, mode, intents);
  }
  const mode: MovingMode = {
    ...base,
    name: "moving",
    trackKind: trackKind(env.model, item.trackId) ?? item.kind,
    start0: item.start,
    duration: item.duration,
    toTrack: item.trackId,
    at: item.start,
    collapse: inSelection && env.selection.length > 1,
  };
  return to(next, mode, intents);
}

/**
 * Frame-aligned limits of a trimmed edge. The start edge stays at or after the timeline start,
 * the media's first frame (unless it is a still) and the previous clip's end, and at least one
 * frame before the end; the end edge stays at least one frame after the start, at or before the
 * media's last frame and the next clip's start. The engine checks all of this again (arch §4.3);
 * these limits only keep the edge from running into a refusal mid-drag.
 */
export function trimLimits(
  model: TimelineModel,
  item: TimelineItem,
  edge: "start" | "end",
): { min: number; max: number } {
  const frame = frameDurationFlicks(model.fps);
  const start = item.start;
  const end = item.start + item.duration;
  let prevEnd = 0;
  let nextStart = Number.POSITIVE_INFINITY;
  for (const other of model.items) {
    if (other.id === item.id || other.trackId !== item.trackId) continue;
    const oEnd = other.start + other.duration;
    if (oEnd <= start) prevEnd = Math.max(prevEnd, oEnd);
    if (other.start >= end) nextStart = Math.min(nextStart, other.start);
  }
  const src = item.source;
  if (edge === "start") {
    let min = prevEnd;
    if (src !== undefined && !src.still) min = Math.max(min, start - src.sourceIn);
    return { min: snapToFrame(min, model.fps, "ceil"), max: end - frame };
  }
  let max = nextStart;
  if (src !== undefined && !src.still && src.mediaDuration !== null) {
    max = Math.min(max, start - src.sourceIn + src.mediaDuration);
  }
  const maxAligned = Number.isFinite(max) ? snapToFrame(max, model.fps, "floor") : max;
  return { min: start + frame, max: maxAligned };
}

// ---- pointer move ---------------------------------------------------------------------------

function onMove(state: InteractionState, e: PointerInput): Transition {
  const m = state.mode;
  switch (m.name) {
    case "idle":
    case "hovering": {
      if (m.name === "hovering" && m.zone === e.hit.zone && m.itemId === e.hit.itemId) {
        return same(state);
      }
      return to(state, hovering(e));
    }
    case "selecting": {
      if (m.marquee === null && !travelled(m, e)) return same(state);
      const rect = marqueeRect(m, e);
      return to(state, { ...m, marquee: rect }, [{ kind: "marquee", rect }]);
    }
    case "moving":
    case "trimming-start":
    case "trimming-end": {
      if (!m.dragging) {
        if (!travelled(m, e)) return same(state);
        return dragTo(state, startDrag(state, m), e, [
          { kind: "begin-gesture", gesture: m.name === "moving" ? "move" : "trim", item: m.item },
        ]);
      }
      return dragTo(state, m, e, []);
    }
    case "scrubbing-ruler": {
      const at = scrubTime(state.env, m.targets, e.hit.time);
      if (at === m.at) return same(state);
      return to(state, { ...m, at }, [{ kind: "seek", at }]);
    }
    case "panning": {
      const k = FLICKS_PER_SECOND / state.env.pxPerSec;
      const scrollX = Math.round(m.scrollX0 - (e.x - m.originX) * k);
      const scrollY = m.scrollY0 - (e.y - m.originY);
      return { state, intents: [{ kind: "scroll", scrollX, scrollY }] };
    }
    case "dropping-media":
      return same(state);
  }
}

function marqueeRect(m: SelectingMode, e: PointerInput): Rect {
  return {
    x: Math.min(m.originX, e.x),
    y: Math.min(m.originY, e.y),
    w: Math.abs(e.x - m.originX),
    h: Math.abs(e.y - m.originY),
  };
}

function startDrag(
  state: InteractionState,
  m: MovingMode | TrimmingMode,
): MovingMode | TrimmingMode {
  const { env } = state;
  if (!env.snapping) return { ...m, dragging: true, targets: [] };
  const item = findItem(env.model, m.item);
  const targets = snapTargets(env.model, {
    exclude: new Set([m.item]),
    excludeGroup: item?.linkedGroup,
    playhead: env.playhead,
  });
  return { ...m, dragging: true, targets };
}

/** Applies pointer `e` to an open drag and emits the update when the snapped result changed. */
function dragTo(
  state: InteractionState,
  m: MovingMode | TrimmingMode,
  e: PointerInput,
  intents: Intent[],
): Transition {
  const { env } = state;
  const unlink = m.unlink || e.altKey;
  const delta = e.hit.time - m.originTime;
  const tolerance = snapTolerance(env.pxPerSec, SNAP_PX);
  const targets = env.snapping ? m.targets : [];
  let next: MovingMode | TrimmingMode;
  let update: Intent | null = null;

  if (m.name === "moving") {
    const raw = Math.max(0, m.start0 + delta);
    const snap = snapSpan(raw, [0, m.duration], targets, tolerance, env.model.fps);
    const at = Math.max(0, snap.at);
    // Across tracks only onto one of the same kind; elsewhere (ruler, headers, below the last
    // track, outside the canvas) the clip keeps its current track.
    const hovered = e.hit.trackId;
    const toTrack =
      hovered !== null && trackKind(env.model, hovered) === m.trackKind ? hovered : m.toTrack;
    next = { ...m, at, toTrack, unlink, guide: snap.guide };
    if (at !== m.at || toTrack !== m.toTrack || unlink !== m.unlink) {
      update = { kind: "move", item: m.item, toTrack, at, unlink };
    }
  } else {
    const raw = m.edge0 + delta;
    const snap = snapSpan(raw, [0], targets, tolerance, env.model.fps);
    const at = m.min <= m.max ? Math.min(m.max, Math.max(m.min, snap.at)) : m.edge0;
    const guide = snap.guide !== null && snap.guide === at ? snap.guide : null;
    next = { ...m, at, unlink, guide };
    if (at !== m.at || unlink !== m.unlink) {
      const edge = m.name === "trimming-start" ? "start" : "end";
      update = { kind: "trim", item: m.item, edge, at, unlink };
    }
  }
  if (update !== null) intents.push(update);
  if (next.guide !== m.guide) {
    intents.push({ kind: "snap-guides", times: next.guide === null ? [] : [next.guide] });
  }
  return to(state, next, intents);
}

function scrubTime(env: InteractionEnv, targets: readonly number[], t: number): number {
  const tolerance = snapTolerance(env.pxPerSec, SNAP_PX);
  return Math.max(0, snapSpan(Math.max(0, t), [0], targets, tolerance, env.model.fps).at);
}

// ---- pointer up -----------------------------------------------------------------------------

function onUp(state: InteractionState, e: PointerInput): Transition {
  const m = state.mode;
  switch (m.name) {
    case "selecting": {
      if (m.marquee !== null || travelled(m, e)) {
        const rect = marqueeRect(m, e);
        return to(state, hovering(e), [
          { kind: "select-rect", rect, additive: m.additive },
          { kind: "marquee", rect: null },
        ]);
      }
      if (m.additive || state.env.selection.length === 0) return to(state, hovering(e));
      return to(select(state, []), hovering(e), [{ kind: "select", ids: [] }]);
    }
    case "moving":
    case "trimming-start":
    case "trimming-end": {
      if (!m.dragging) {
        if (m.name === "moving" && m.collapse) {
          return to(select(state, [m.item]), hovering(e), [{ kind: "select", ids: [m.item] }]);
        }
        return to(state, hovering(e));
      }
      const moved = dragTo(state, m, e, []);
      const intents: Intent[] = [...moved.intents.filter((i) => i.kind !== "snap-guides")];
      intents.push({ kind: "commit-gesture" });
      if (m.guide !== null) intents.push({ kind: "snap-guides", times: [] });
      return to(state, hovering(e), intents);
    }
    case "scrubbing-ruler": {
      const at = scrubTime(state.env, m.targets, e.hit.time);
      return to(state, hovering(e), at === m.at ? NONE : [{ kind: "seek", at }]);
    }
    case "panning":
      return to(state, hovering(e));
    default:
      return same(state);
  }
}

// ---- cancel, abort, keys --------------------------------------------------------------------

/** Esc or a lost pointer: roll the gesture back and return to idle. */
function cancel(state: InteractionState): Transition {
  const m = state.mode;
  switch (m.name) {
    case "moving":
    case "trimming-start":
    case "trimming-end": {
      if (!m.dragging) return to(state, IDLE);
      const intents: Intent[] = [{ kind: "cancel-gesture" }];
      if (m.guide !== null) intents.push({ kind: "snap-guides", times: [] });
      return to(state, IDLE, intents);
    }
    case "selecting":
      return to(state, IDLE, m.marquee === null ? NONE : [{ kind: "marquee", rect: null }]);
    case "scrubbing-ruler":
      return to(state, IDLE, m.at === m.origin ? NONE : [{ kind: "seek", at: m.origin }]);
    case "panning":
      return to(state, IDLE, [{ kind: "scroll", scrollX: m.scrollX0, scrollY: m.scrollY0 }]);
    case "dropping-media":
      return leaveDrop(state);
    default:
      return same(state);
  }
}

// ---- dropping media from the bin ------------------------------------------------------------

/** The kind of track a dragged asset goes on: pictures on a media track, sound on an audio one. */
const wantedTrack = (media: MediaDrag): TrackKind => (media.kind === "audio" ? "audio" : "media");

/** The track under the pointer when the asset fits it, else the first unlocked one that does. */
function dropTrack(
  model: TimelineModel,
  hitTrackId: string | null,
  media: MediaDrag,
): string | null {
  const want = wantedTrack(media);
  if (hitTrackId !== null && trackKind(model, hitTrackId) === want) return hitTrackId;
  return model.tracks.find((t) => t.kind === want && !t.locked)?.id ?? null;
}

/** The snapped start for a clip of `media` whose left edge is at the pointer time `raw`. */
function dropStart(
  env: InteractionEnv,
  targets: readonly number[],
  media: MediaDrag,
  raw: number,
): { at: number; guide: number | null } {
  const length = media.duration ?? STILL_DROP_DURATION;
  const tolerance = snapTolerance(env.pxPerSec, SNAP_PX);
  const out = snapSpan(Math.max(0, raw), [0, length], targets, tolerance, env.model.fps);
  return { at: Math.max(0, out.at), guide: out.guide };
}

function onMediaOver(
  state: InteractionState,
  e: MediaDragPoint & { readonly media: MediaDrag },
): Transition {
  const m = state.mode;
  if (m.name !== "dropping-media" && isBusy(state)) return same(state);
  const { env } = state;
  const trackId = dropTrack(env.model, e.hit.trackId, e.media);
  if (trackId === null) {
    // Nothing to land on (no track of the right kind): clear any earlier preview.
    return m.name === "dropping-media" ? leaveDrop(state) : same(state);
  }
  const targets =
    m.name === "dropping-media" && m.media.id === e.media.id
      ? m.targets
      : env.snapping
        ? snapTargets(env.model, { playhead: env.playhead })
        : [];
  const { at } = dropStart(env, targets, e.media, e.hit.time);
  if (
    m.name === "dropping-media" &&
    m.media.id === e.media.id &&
    m.at === at &&
    m.trackId === trackId
  ) {
    return same(state);
  }
  const mode: DroppingMediaMode = {
    name: "dropping-media",
    media: e.media,
    at,
    trackId,
    targets,
    guide: at,
  };
  return to(state, mode, [{ kind: "snap-guides", times: [at] }]);
}

function onMediaDrop(
  state: InteractionState,
  e: MediaDragPoint & { readonly media: MediaDrag },
): Transition {
  const m = state.mode;
  if (m.name !== "dropping-media" && isBusy(state)) return same(state);
  const { env } = state;
  const trackId = dropTrack(env.model, e.hit.trackId, e.media);
  const clear: Intent[] = m.name === "dropping-media" ? [{ kind: "snap-guides", times: [] }] : [];
  if (trackId === null) return to(state, IDLE, clear);
  const targets =
    m.name === "dropping-media"
      ? m.targets
      : env.snapping
        ? snapTargets(env.model, { playhead: env.playhead })
        : [];
  const { at } = dropStart(env, targets, e.media, e.hit.time);
  return to(state, IDLE, [{ kind: "insert", media: e.media.id, trackId, at }, ...clear]);
}

function leaveDrop(state: InteractionState): Transition {
  return to(state, IDLE, [{ kind: "snap-guides", times: [] }]);
}

/** The host already rolled the gesture back (engine refusal): only leave the gesture state. */
function aborted(state: InteractionState): Transition {
  const m = state.mode;
  if (m.name !== "moving" && m.name !== "trimming-start" && m.name !== "trimming-end") {
    return same(state);
  }
  return to(state, IDLE, m.guide === null ? NONE : [{ kind: "snap-guides", times: [] }]);
}

/** What a key does in the timeline panel. */
export type TimelineKeyAction = "cancel" | "split" | "delete" | "toggle-snapping" | "rename-track";

/**
 * The timeline's own keys, as one table: `onKey` below reads it, and the editor's list of
 * shortcuts (Settings > Keyboard shortcuts) is generated from it, so the two cannot drift.
 * `keys` are `KeyboardEvent.key` values; a letter matches either case. None takes a modifier.
 */
export const TIMELINE_KEYS: readonly {
  readonly action: TimelineKeyAction;
  readonly keys: readonly string[];
}[] = [
  { action: "cancel", keys: ["Escape"] },
  { action: "split", keys: ["s"] },
  { action: "delete", keys: ["Delete", "Backspace"] },
  { action: "toggle-snapping", keys: ["n"] },
  { action: "rename-track", keys: ["F2"] },
];

/** The action a key press names in {@link TIMELINE_KEYS}, or `null`. */
export function timelineKeyAction(key: string): TimelineKeyAction | null {
  const lower = key.length === 1 ? key.toLowerCase() : key;
  return TIMELINE_KEYS.find((entry) => entry.keys.includes(lower))?.action ?? null;
}

function onKey(state: InteractionState, e: KeyInput): Transition {
  const action = timelineKeyAction(e.key);
  if (action === "cancel") return cancel(state);
  if (e.ctrlKey === true || e.metaKey === true || e.altKey === true) return same(state);
  switch (action) {
    case "split":
      return split(state);
    case "delete":
      return remove(state);
    case "toggle-snapping":
      return toggleSnapping(state);
    case "rename-track":
      return isBusy(state) ? same(state) : to(state, state.mode, [{ kind: "rename-track" }]);
    default:
      return same(state);
  }
}

function toggleSnapping(state: InteractionState): Transition {
  const on = !state.env.snapping;
  return {
    state: { ...state, env: { ...state.env, snapping: on } },
    intents: [{ kind: "set-snapping", on }],
  };
}

/**
 * Split at the playhead (frame-aligned): every selected item that strictly contains it, one per
 * linked group (the engine splits partners itself); with nothing selected, the clip under the
 * playhead on the topmost track that has one.
 */
function split(state: InteractionState): Transition {
  if (isBusy(state)) return same(state);
  const { model, selection, playhead } = state.env;
  const at = snapToFrame(playhead, model.fps);
  const inside = (it: TimelineItem): boolean => it.start < at && at < it.start + it.duration;
  const items: string[] = [];
  if (selection.length > 0) {
    const groups = new Set<string>();
    for (const id of selection) {
      const it = findItem(model, id);
      if (it === undefined || !inside(it)) continue;
      if (it.linkedGroup !== undefined) {
        if (groups.has(it.linkedGroup)) continue;
        groups.add(it.linkedGroup);
      }
      items.push(it.id);
    }
  } else {
    for (const track of model.tracks) {
      const hit = model.items.find((it) => it.trackId === track.id && inside(it));
      if (hit !== undefined) {
        items.push(hit.id);
        break;
      }
    }
  }
  return items.length === 0 ? same(state) : { state, intents: [{ kind: "split", items, at }] };
}

function remove(state: InteractionState): Transition {
  if (isBusy(state)) return same(state);
  const { model, selection } = state.env;
  const items = selection.filter((id) => findItem(model, id) !== undefined);
  return items.length === 0 ? same(state) : { state, intents: [{ kind: "delete", items }] };
}
