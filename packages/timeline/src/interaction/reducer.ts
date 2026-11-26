// SPDX-License-Identifier: GPL-3.0-or-later
// The timeline interaction state machine (WP-2A.08, arch §16, timeline spec §5-§7): the transition
// function `transition(state, event, deps) -> { state, intents, edges }`. Hand-rolled and
// XState-style: one explicit state per gesture, every edge named in the state chart of
// `packages/timeline/README.md`. It reads the host only through `deps` (hit-testing, snapping:
// queries) and speaks only in intents; nothing here touches the engine, the DOM or React.
//
// Gestures (spec §3.9, §5). Past the drag threshold a gesture opens one `gesture` transaction
// (`begin_tx`); each animation frame sends at most one update, the command for where the pointer
// is now, which the engine applies with `_meta.replace` (the gesture equals its last update);
// release flushes the last update and commits, and Esc, a lost pointer or a lost window cancel
// and roll everything back. Every `begin_tx` is closed by exactly one `commit_tx` or `cancel_tx`.
// A refused update leaves the previous one in place: the machine shows a "not allowed" or
// "locked" cursor and goes on; it never sends the same update again on its own.
import type { ItemId, MediaId, SequenceId, TrackId } from "@cutweave/protocol";
import type { Rect, SnapGuide, TimelineModel } from "../model.ts";
import { FLICKS_PER_SECOND, frameDurationFlicks, snapToFrame } from "../time.ts";
import {
  AUTO_SCROLL_EDGE_PX,
  AUTO_SCROLL_MAX_PX,
  DRAG_THRESHOLD_PX,
  NUDGE_FAR_FRAMES,
  STILL_DROP_DURATION,
  ZOOM_CLICK_FACTOR,
} from "./constants.ts";
import { endOf, groupOf, isLocked, isMain, isUnbounded, itemOf, itemsOn, trackOf } from "./doc.ts";
import { MODIFIER_KEYS, machineKeyAction } from "./keys.ts";
import { dropTrack, itemAt, liftedCuts, nearestCut, neighbour, overlaps } from "./placement.ts";
import {
  clickUnit,
  editTargets,
  isPartial,
  marqueeSelection,
  sameIds,
  toggled,
} from "./selection.ts";
import {
  type DragOrigin,
  type DroppingMode,
  type GestureMode,
  type HeldKeys,
  type HoverMode,
  isMachineBusy,
  type MachineMode,
  type MachineState,
  type MovingMode,
  type PressedMode,
  type PressTarget,
  type StateName,
  type TxState,
} from "./state.ts";
import type {
  CancelReason,
  Cursor,
  DropPreview,
  DroppedMedia,
  GestureEdit,
  HitResult,
  MachineAction,
  MachineCommand,
  MachineDeps,
  MachineEvent,
  MachineIntent,
  Modifiers,
  NoticeCode,
  PointerPoint,
  SnapOutcome,
  Tool,
} from "./types.ts";

/** One edge of the state chart, as taken. */
export interface ChartEdge {
  readonly from: StateName;
  readonly to: StateName;
  readonly label: string;
}

export interface MachineTransition {
  readonly state: MachineState;
  readonly intents: readonly MachineIntent[];
  /** The chart edges this event took (usually none or one). */
  readonly edges: readonly ChartEdge[];
}

// ---- the step: a working copy of the state and what the event produced ----------------------

class Step {
  s: MachineState;
  readonly intents: MachineIntent[] = [];
  readonly edges: ChartEdge[] = [];

  constructor(
    state: MachineState,
    readonly deps: MachineDeps,
  ) {
    this.s = state;
  }

  emit(intent: MachineIntent): void {
    this.intents.push(intent);
  }

  /** Enters `mode`, recording the chart edge when the state changes. */
  go(mode: MachineMode, label: string): void {
    const from = this.s.mode.name;
    if (mode.name !== from) this.edges.push({ from, to: mode.name, label });
    this.s = { ...this.s, mode };
  }

  /** Replaces the mode's data without leaving the state. */
  set(mode: MachineMode): void {
    this.s = { ...this.s, mode };
  }

  env(patch: Partial<MachineState["env"]>): void {
    this.s = { ...this.s, env: { ...this.s.env, ...patch } };
  }

  keys(keys: HeldKeys): void {
    const k = this.s.keys;
    if (
      k.shift === keys.shift &&
      k.alt === keys.alt &&
      k.ctrl === keys.ctrl &&
      k.meta === keys.meta
    )
      return;
    this.s = { ...this.s, keys };
  }

  pointer(x: number, y: number): void {
    this.s = { ...this.s, pointer: { x, y } };
  }

  cursor(cursor: Cursor): void {
    if (cursor === this.s.cursor) return;
    this.s = { ...this.s, cursor };
    this.emit({ type: "set_cursor", cursor });
  }

  guides(guides: readonly SnapGuide[]): void {
    if (sameGuides(guides, this.s.guides)) return;
    this.s = { ...this.s, guides };
    this.emit({ type: "snap_guides", guides });
  }

  preview(preview: DropPreview | null): void {
    if (samePreview(preview, this.s.preview)) return;
    this.s = { ...this.s, preview };
    this.emit({ type: "preview", preview });
  }

  marquee(rect: Rect | null): void {
    const cur = this.s.marquee;
    if (rect === cur) return;
    if (
      rect !== null &&
      cur !== null &&
      rect.x === cur.x &&
      rect.y === cur.y &&
      rect.w === cur.w &&
      rect.h === cur.h
    )
      return;
    this.s = { ...this.s, marquee: rect };
    this.emit({ type: "marquee", rect });
  }

  select(ids: readonly string[]): void {
    const cur = this.s.env.selection;
    if (ids.length === cur.length && ids.every((id, i) => cur[i] === id)) return;
    this.env({ selection: ids });
    this.emit({ type: "set_selection", ids });
  }

  notice(code: NoticeCode): void {
    this.emit({ type: "notice", code });
  }

  result(): MachineTransition {
    return { state: this.s, intents: this.intents, edges: this.edges };
  }
}

const guideTime = (g: SnapGuide): number => (typeof g === "number" ? g : g.t);
const guideKind = (g: SnapGuide): string => (typeof g === "number" ? "" : g.kind);

function sameGuides(a: readonly SnapGuide[], b: readonly SnapGuide[]): boolean {
  return (
    a.length === b.length &&
    a.every((g, i) => {
      const h = b[i];
      return h !== undefined && guideTime(g) === guideTime(h) && guideKind(g) === guideKind(h);
    })
  );
}

function samePreview(a: DropPreview | null, b: DropPreview | null): boolean {
  if (a === null || b === null) return a === b;
  return a.trackId === b.trackId && a.start === b.start && a.end === b.end && a.mode === b.mode;
}

const keyOf = (c: MachineCommand | null): string | null =>
  c === null ? null : `${c.name}${JSON.stringify(c.args)}`;

function keysOf(e: Modifiers): HeldKeys {
  return {
    shift: e.shift === true,
    alt: e.alt === true,
    ctrl: e.ctrl === true,
    meta: e.meta === true,
  };
}

/** The held keys after `key` went down or up (a modifier key's own flag may be missing). */
function keysAfter(e: Modifiers & { readonly key: string }, down: boolean): HeldKeys {
  const k = keysOf(e);
  switch (e.key) {
    case "Shift":
      return { ...k, shift: down };
    case "Alt":
      return { ...k, alt: down };
    case "Control":
      return { ...k, ctrl: down };
    case "Meta":
      return { ...k, meta: down };
    default:
      return k;
  }
}

// ---- entry point --------------------------------------------------------------------------------

/** The transition function. Pure apart from the `deps` queries it makes. */
export function transition(
  state: MachineState,
  event: MachineEvent,
  deps: MachineDeps,
): MachineTransition {
  const st = new Step(state, deps);
  switch (event.type) {
    case "env":
      onEnv(st, event.env);
      break;
    case "pointer.move":
      onMove(st, event);
      break;
    case "pointer.down":
      onDown(st, event, event.button ?? 0);
      break;
    case "pointer.up":
      onUp(st, event);
      break;
    case "pointer.leave":
      onLeave(st);
      break;
    case "pointer.cancel":
      cancel(st, "pointer_cancel");
      break;
    case "blur":
      cancel(st, "blur");
      break;
    case "key.down":
      onKey(st, event, true);
      break;
    case "key.up":
      onKey(st, event, false);
      break;
    case "action":
      act(st, event.action);
      break;
    case "media.over":
      onMediaOver(st, event);
      break;
    case "media.drop":
      onMediaDrop(st, event);
      break;
    case "media.leave":
      if (st.s.mode.name === "dropping") endDrop(st, "media_leave", "media.leave");
      break;
    case "frame":
      onFrame(st);
      break;
    case "engine.result":
      onResult(st, event);
      break;
    case "engine.cancelled":
      if (openGesture(st) === event.gesture) abort(st, "engine");
      break;
  }
  return st.result();
}

/** Whether the state wants an animation frame (coalesced engine traffic or auto-scroll is due). */
export function wantsFrame(state: MachineState): boolean {
  const m = state.mode;
  if (m.name === "scrubbing") return m.pending !== null;
  if (!("tx" in m)) return false;
  if (m.tx.pending !== null && m.tx.pendingKey !== m.tx.sentKey) return true;
  return "scroll" in m && m.scroll !== null;
}

function openGesture(st: Step): number | null {
  const m = st.s.mode;
  return "tx" in m ? m.tx.gesture : null;
}

// ---- snapping -----------------------------------------------------------------------------------

function snapAt(
  st: Step,
  model: TimelineModel,
  exclude: ReadonlySet<string> | null,
  raw: number,
  offsets: readonly number[],
  withoutPlayhead = false,
): SnapOutcome {
  const { env, keys } = st.s;
  return st.deps.snap.snap({
    model,
    exclude,
    raw,
    offsets,
    pxPerSec: env.pxPerSec,
    thresholdPx: env.snapThresholdPx,
    kinds: withoutPlayhead ? env.snapKinds.filter((k) => k !== "playhead") : env.snapKinds,
    suspended: !env.snapping || keys.ctrl || keys.meta,
    playhead: env.playhead,
    inOut: env.inOut,
  });
}

// ---- hover and cursors --------------------------------------------------------------------------

const CLIP_ZONES: ReadonlySet<string> = new Set(["body", "left-edge", "right-edge", "keyframe"]);
const LANE_ZONES: ReadonlySet<string> = new Set([...CLIP_ZONES, "lane"]);

function hoverCursor(st: Step, hit: HitResult): Cursor {
  const { env, keys } = st.s;
  if (hit.zone === "ruler" || hit.zone === "playhead") return "scrub";
  if (hit.zone.startsWith("header")) return "auto";
  if (!LANE_ZONES.has(hit.zone)) return "default";
  const tool: Tool = env.tool;
  if (tool === "hand") return "grab";
  if (tool === "zoom") return keys.alt ? "zoom-out" : "zoom-in";
  const item = itemOf(env.model, hit.itemId);
  if (item === undefined || !CLIP_ZONES.has(hit.zone)) return "default";
  if (tool === "razor") return isLocked(env.model, item.trackId) ? "not-allowed" : "razor";
  if (isLocked(env.model, item.trackId)) return "not-allowed";
  if (env.heldBy.has(item.id)) return "locked";
  const edge = hit.zone === "left-edge" ? "start" : hit.zone === "right-edge" ? "end" : null;
  switch (tool) {
    case "select":
      if (edge !== null) return edge === "start" ? "trim-start" : "trim-end";
      return keys.alt ? "copy" : "move";
    case "ripple":
      if (edge === null) return "default";
      return edge === "start" ? "ripple-start" : "ripple-end";
    case "roll":
      return edge === null ? "default" : "roll";
    case "slip":
      return isUnbounded(item) ? "not-allowed" : "slip";
    case "slide":
      return "slide";
    default:
      return "default";
  }
}

function gestureCursor(m: GestureMode | DroppingMode): Cursor {
  if (m.tx.refusal !== null) return m.tx.refusal.code === "conflict" ? "locked" : "not-allowed";
  switch (m.name) {
    case "moving":
      return m.copy ? "copy" : "move";
    case "trimming":
      return m.edge === "start" ? "trim-start" : "trim-end";
    case "ripple_trimming":
      return m.edge === "start" ? "ripple-start" : "ripple-end";
    case "rolling":
      return "roll";
    case "slipping":
      return "slip";
    case "sliding":
      return "slide";
    case "keyframe_dragging":
      return "move";
    case "dropping":
      return "copy";
  }
}

/** The razor's cut under the pointer: the clip and the snapped time (spec §6: frame-aligned). */
function razorAt(st: Step, hit: HitResult): { item: string | null; at: number; guide: SnapGuide } {
  const model = st.s.env.model;
  const s = snapAt(st, model, null, Math.max(0, hit.time), [0]);
  const at = Math.max(0, s.t);
  const item = CLIP_ZONES.has(hit.zone) && itemOf(model, hit.itemId) ? hit.itemId : null;
  return { item, at, guide: s.target ?? at };
}

/** Recomputes the hover state, cursor and razor guide at the pointer. */
function hoverUpdate(st: Step): void {
  const p = st.s.pointer;
  if (p === null) return;
  const hit = st.deps.hitTest(p.x, p.y);
  const m = st.s.mode;
  const next: HoverMode = { name: "hover", zone: hit.zone, item: hit.itemId };
  if (m.name !== "hover" || m.zone !== next.zone || m.item !== next.item) st.set(next);
  st.cursor(hoverCursor(st, hit));
  if (st.s.env.tool === "razor" && LANE_ZONES.has(hit.zone)) st.guides([razorAt(st, hit).guide]);
  else st.guides([]);
}

/** Leaves whatever state for hover at the pointer (release paths). */
function toHover(st: Step, label: string): void {
  st.go({ name: "hover", zone: "none", item: null }, label);
  st.preview(null);
  st.marquee(null);
  hoverUpdate(st);
}

function toIdle(st: Step, label: string): void {
  st.go({ name: "idle" }, label);
  st.guides([]);
  st.preview(null);
  st.marquee(null);
  st.cursor("default");
}

/** A modifier changed: re-read what depends on it at the current pointer. */
function refresh(st: Step): void {
  const m = st.s.mode;
  switch (m.name) {
    case "hover":
      hoverUpdate(st);
      return;
    case "moving":
    case "trimming":
    case "ripple_trimming":
    case "rolling":
    case "slipping":
    case "sliding":
    case "keyframe_dragging":
      evaluate(st);
      return;
    case "razor": {
      const p = st.s.pointer;
      if (p === null) return;
      const r = razorAt(st, st.deps.hitTest(p.x, p.y));
      st.set({ ...m, item: r.item, at: r.at });
      st.guides([r.guide]);
      return;
    }
    case "zooming":
      st.cursor(st.s.keys.alt ? "zoom-out" : "zoom-in");
      return;
    default:
      return;
  }
}

// ---- pointer ------------------------------------------------------------------------------------

const travelled = (o: { x: number; y: number }, p: { x: number; y: number }): boolean =>
  Math.abs(p.x - o.x) >= DRAG_THRESHOLD_PX || Math.abs(p.y - o.y) >= DRAG_THRESHOLD_PX;

function onMove(st: Step, e: PointerPoint): void {
  st.keys(keysOf(e));
  st.pointer(e.x, e.y);
  const m = st.s.mode;
  switch (m.name) {
    case "idle":
      st.go({ name: "hover", zone: "none", item: null }, "pointer.over");
      hoverUpdate(st);
      return;
    case "hover":
      hoverUpdate(st);
      return;
    case "pressed":
      pressedMove(st, m, e);
      return;
    case "selecting":
      if (!m.dragging && !travelled(m, e)) return;
      st.set({ ...m, dragging: true });
      st.marquee(rectBetween(m, e));
      return;
    case "scrubbing": {
      const s = scrubAt(st, st.deps.hitTest(e.x, e.y).time);
      if (s.t !== (m.pending ?? m.sent)) st.set({ ...m, pending: s.t });
      st.guides(s.target === null ? [] : [s.target]);
      return;
    }
    case "panning": {
      const k = FLICKS_PER_SECOND / st.s.env.pxPerSec;
      const scrollX = Math.max(0, Math.round(m.scrollX0 - (e.x - m.x) * k));
      const scrollY = Math.max(0, m.scrollY0 - (e.y - m.y));
      if (scrollX !== st.s.env.scrollX || scrollY !== st.s.env.scrollY) {
        st.env({ scrollX, scrollY });
        st.emit({ type: "scroll_to", scrollX, scrollY });
      }
      return;
    }
    case "zooming": {
      if (!m.dragging && !travelled(m, e)) return;
      st.set({ ...m, dragging: true });
      st.marquee(zoomBand(st, m.x, e.x));
      return;
    }
    case "razor": {
      const r = razorAt(st, st.deps.hitTest(e.x, e.y));
      st.set({ ...m, item: r.item, at: r.at });
      st.guides([r.guide]);
      return;
    }
    case "dropping":
      return; // a card drag reports its positions as `media.over`
    default:
      evaluate(st);
  }
}

function rectBetween(o: { x: number; y: number }, p: { x: number; y: number }): Rect {
  return {
    x: Math.min(o.x, p.x),
    y: Math.min(o.y, p.y),
    w: Math.abs(p.x - o.x),
    h: Math.abs(p.y - o.y),
  };
}

function zoomBand(st: Step, x0: number, x1: number): Rect {
  const l = st.deps.lanes();
  return { x: Math.min(x0, x1), y: l.top, w: Math.abs(x1 - x0), h: Math.max(0, l.bottom - l.top) };
}

function onLeave(st: Step): void {
  if (st.s.mode.name === "hover") toIdle(st, "pointer.leave");
}

function onDown(st: Step, e: PointerPoint, button: number): void {
  st.keys(keysOf(e));
  st.pointer(e.x, e.y);
  const m = st.s.mode;
  if (m.name === "dropping") {
    // A card drag delivers no pointer events: a press means it ended without a drop here.
    endDrop(st, "media_leave", "cancel");
    return;
  }
  if (isMachineBusy(st.s)) return; // a second button during a gesture
  if (m.name === "idle") st.go({ name: "hover", zone: "none", item: null }, "pointer.over");
  const hit = st.deps.hitTest(e.x, e.y);
  st.set({ name: "hover", zone: hit.zone, item: hit.itemId });
  const { env } = st.s;
  if (button === 1) {
    startPan(st, e);
    return;
  }
  // The secondary button is the host's context menu (WP-2H.12), never a gesture.
  if (button !== 0) return;
  if (hit.zone === "ruler" || hit.zone === "playhead") {
    startScrub(st, hit);
    return;
  }
  if (!LANE_ZONES.has(hit.zone) && hit.zone !== "none") return; // headers are the host's
  switch (env.tool) {
    case "hand":
      startPan(st, e);
      return;
    case "zoom":
      st.go(
        { name: "zooming", x: e.x, y: e.y, time: hit.time, dragging: false },
        "pointer.down [zoom]",
      );
      st.cursor(st.s.keys.alt ? "zoom-out" : "zoom-in");
      return;
    case "razor": {
      const r = razorAt(st, hit);
      st.go({ name: "razor", item: r.item, at: r.at }, "pointer.down [razor]");
      st.guides([r.guide]);
      st.cursor("razor");
      return;
    }
    default:
      if (CLIP_ZONES.has(hit.zone) && itemOf(env.model, hit.itemId) !== undefined) {
        pressItem(st, e, hit);
        return;
      }
      st.go(
        { name: "selecting", x: e.x, y: e.y, additive: st.s.keys.shift, dragging: false },
        "pointer.down [lane]",
      );
      st.guides([]);
      st.cursor("default");
  }
}

function startPan(st: Step, e: PointerPoint): void {
  const { env } = st.s;
  st.go(
    { name: "panning", x: e.x, y: e.y, scrollX0: env.scrollX, scrollY0: env.scrollY },
    "pointer.down [hand]",
  );
  st.guides([]);
  st.cursor("grabbing");
}

function scrubAt(st: Step, time: number): SnapOutcome {
  const s = snapAt(st, st.s.env.model, null, Math.max(0, time), [0], true);
  return { t: Math.max(0, s.t), target: s.target };
}

function startScrub(st: Step, hit: HitResult): void {
  const origin = st.s.env.playhead;
  const s = scrubAt(st, hit.time);
  st.go({ name: "scrubbing", from: origin, sent: s.t, pending: null }, "pointer.down [ruler]");
  st.env({ playhead: s.t });
  st.emit({ type: "seek", t: s.t });
  st.guides(s.target === null ? [] : [s.target]);
  st.cursor("scrub");
}

function pressItem(st: Step, e: PointerPoint, hit: HitResult): void {
  const { env, keys } = st.s;
  const id = hit.itemId as string;
  const unit = clickUnit(env.model, id, { alt: keys.alt, linked: env.linkedSelection });
  if (keys.shift) {
    // Shift+click adds or removes the clip (its group); it never starts a drag.
    st.select(toggled(env.selection, unit));
    return;
  }
  let collapse: readonly string[] | null = null;
  if (!env.selection.includes(id)) st.select(unit);
  else if (!sameIds(env.selection, unit)) collapse = unit;
  const target = pressTarget(st, hit);
  st.go(
    { name: "pressed", x: e.x, y: e.y, time: hit.time, target, collapse, dragged: false },
    "pointer.down [clip]",
  );
  st.guides([]);
  if (target.kind === "none") st.cursor("not-allowed");
}

function pressTarget(st: Step, hit: HitResult): PressTarget {
  const { env, keys } = st.s;
  const model = env.model;
  const item = itemOf(model, hit.itemId);
  const id = hit.itemId as string;
  if (item === undefined || isLocked(model, item.trackId))
    return { kind: "none", item: id, notice: null };
  const partial = isPartial(model, env.selection, id);
  const blocked: PressTarget = { kind: "none", item: id, notice: "partial_group" };
  if (
    hit.zone === "keyframe" &&
    env.tool === "select" &&
    st.deps.keyframes !== undefined &&
    hit.keyframeTime !== null
  ) {
    return partial ? blocked : { kind: "keyframe", item: id, keyframeTime: hit.keyframeTime };
  }
  const edge = hit.zone === "left-edge" ? "start" : hit.zone === "right-edge" ? "end" : null;
  const unlink = keys.alt || partial;
  switch (env.tool) {
    case "select":
      return edge === null
        ? { kind: "body", item: id }
        : { kind: "edge", item: id, edge, gesture: "trim", unlink };
    case "ripple":
      return edge === null
        ? { kind: "none", item: id, notice: null }
        : { kind: "edge", item: id, edge, gesture: "ripple_trim", unlink };
    case "roll": {
      if (edge === null) return { kind: "none", item: id, notice: null };
      const other = neighbour(model, item, edge);
      // An edge with a gap beside it has nothing to roll against: it trims (Premiere does too).
      if (other === undefined) return { kind: "edge", item: id, edge, gesture: "trim", unlink };
      if (partial || isPartial(model, env.selection, other.id)) return blocked;
      return edge === "end"
        ? { kind: "roll", left: id, right: other.id }
        : { kind: "roll", left: other.id, right: id };
    }
    case "slip":
      if (isUnbounded(item)) return { kind: "none", item: id, notice: null };
      return partial ? blocked : { kind: "slip", item: id };
    case "slide":
      return partial ? blocked : { kind: "slide", item: id };
    default:
      return { kind: "none", item: id, notice: null };
  }
}

function pressedMove(st: Step, m: PressedMode, p: PointerPoint): void {
  if (!travelled(m, p)) return;
  const t = m.target;
  if (t.kind === "none") {
    if (!m.dragged) {
      if (t.notice !== null) st.notice(t.notice);
      st.set({ ...m, dragged: true });
    }
    return;
  }
  beginDrag(st, m, t);
}

// ---- gestures -----------------------------------------------------------------------------------

/** The items that move with `ids` (their linked partners, unless `unlink`). */
function moversOf(model: TimelineModel, ids: readonly string[], unlink: boolean): Set<string> {
  const out = new Set<string>();
  for (const id of ids) for (const m of unlink ? [id] : groupOf(model, id)) out.add(m);
  return out;
}

function newTx(gesture: number, edit: GestureEdit, noop: MachineCommand | null): TxState {
  return {
    gesture,
    edit,
    pending: null,
    pendingKey: null,
    sentKey: keyOf(noop),
    sent: false,
    refusal: null,
    clamped: false,
  };
}

function beginDrag(st: Step, m: PressedMode, t: Exclude<PressTarget, { kind: "none" }>): void {
  const { env } = st.s;
  const model = env.model;
  const gesture = st.s.lastGesture + 1;
  st.s = { ...st.s, lastGesture: gesture };
  const origin = (exclude: ReadonlySet<string> | null): DragOrigin => ({
    x: m.x,
    y: m.y,
    time: m.time,
    snapshot: model,
    exclude,
  });
  let mode: GestureMode;
  let label: string;
  switch (t.kind) {
    case "body": {
      const copy = st.s.keys.alt;
      const targets = editTargets(model, env.selection);
      // The pressed clip leads: it stands for its group among the targets.
      const group = new Set(groupOf(model, t.item));
      const rest = targets.items.filter((id) => (targets.partial ? id !== t.item : !group.has(id)));
      const items = [t.item, ...rest];
      const unlink = targets.partial;
      const movers = moversOf(model, items, unlink);
      const primary = itemOf(model, t.item);
      if (primary === undefined) return;
      const offsets = new Set<number>();
      let minStart = primary.start;
      for (const id of movers) {
        const it = itemOf(model, id);
        if (it === undefined) continue;
        offsets.add(it.start - primary.start);
        offsets.add(endOf(it) - primary.start);
        minStart = Math.min(minStart, it.start);
      }
      const moving: MovingMode = {
        name: "moving",
        tx: newTx(gesture, copy ? "duplicate" : "move", null),
        origin: origin(copy ? null : movers),
        scroll: null,
        copy,
        items,
        unlink,
        primary: t.item,
        homeTrack: primary.trackId,
        start0: primary.start,
        duration: primary.duration,
        minStart: primary.start - minStart,
        offsets: [...offsets].sort((a, b) => a - b),
        trackChange: !copy && items.every((id) => itemOf(model, id)?.trackId === primary.trackId),
        toTrack: primary.trackId,
      };
      const noop = copy
        ? null
        : moveCommand(moving, primary.start, primary.trackId, isMain(model, primary.trackId));
      mode = { ...moving, tx: newTx(gesture, moving.tx.edit, noop) };
      label = "threshold [move]";
      break;
    }
    case "edge": {
      const item = itemOf(model, t.item);
      if (item === undefined) return;
      const edge0 = t.edge === "start" ? item.start : endOf(item);
      const name = t.gesture === "trim" ? "trimming" : "ripple_trimming";
      const base = { item: t.item, edge: t.edge, edge0, unlink: t.unlink };
      mode = {
        name,
        tx: newTx(gesture, t.gesture, trimCommand(name, base, edge0)),
        origin: origin(moversOf(model, [t.item], t.unlink)),
        scroll: null,
        ...base,
      };
      label = t.gesture === "trim" ? "threshold [trim]" : "threshold [ripple]";
      break;
    }
    case "roll": {
      const left = itemOf(model, t.left);
      if (left === undefined) return;
      const cut0 = endOf(left);
      mode = {
        name: "rolling",
        tx: newTx(gesture, "roll", rollCommand(t.left, t.right, cut0)),
        origin: origin(moversOf(model, [t.left, t.right], false)),
        scroll: null,
        left: t.left,
        right: t.right,
        cut0,
      };
      label = "threshold [roll]";
      break;
    }
    case "slip":
      mode = {
        name: "slipping",
        tx: newTx(gesture, "slip", slipCommand(t.item, 0)),
        origin: origin(null),
        scroll: null,
        item: t.item,
      };
      label = "threshold [slip]";
      break;
    case "slide": {
      const item = itemOf(model, t.item);
      if (item === undefined) return;
      mode = {
        name: "sliding",
        tx: newTx(gesture, "slide", slideCommand(t.item, 0)),
        origin: origin(moversOf(model, [t.item], false)),
        scroll: null,
        item: t.item,
        start0: item.start,
        duration: item.duration,
      };
      label = "threshold [slide]";
      break;
    }
    case "keyframe":
      mode = {
        name: "keyframe_dragging",
        tx: newTx(
          gesture,
          "keyframe",
          st.deps.keyframes?.command(t.item, t.keyframeTime, t.keyframeTime) ?? null,
        ),
        origin: origin(null),
        scroll: null,
        item: t.item,
        keyframeTime: t.keyframeTime,
      };
      label = "threshold [keyframe]";
      break;
  }
  st.emit({ type: "begin_tx", gesture, edit: mode.tx.edit });
  st.go(mode, label);
  evaluate(st);
}

function moveCommand(m: MovingMode, at: number, toTrack: string, main: boolean): MachineCommand {
  return {
    name: "item.move",
    args: {
      items: m.items as ItemId[],
      start: at,
      ...(toTrack !== m.homeTrack && { to_track: toTrack as TrackId }),
      mode: main ? "insert" : "new_track",
      ...(m.unlink && { unlink: true }),
    },
  };
}

function trimCommand(
  name: "trimming" | "ripple_trimming",
  m: { item: string; edge: "start" | "end"; unlink: boolean },
  to: number,
): MachineCommand {
  const args = {
    item: m.item as ItemId,
    edge: m.edge,
    to,
    clamp: true,
    ...(m.unlink && { unlink: true }),
  };
  return name === "trimming" ? { name: "clip.trim", args } : { name: "clip.ripple_trim", args };
}

const rollCommand = (left: string, right: string, to: number): MachineCommand => ({
  name: "clip.roll",
  args: { left: left as ItemId, right: right as ItemId, to, clamp: true },
});

const slipCommand = (item: string, delta: number): MachineCommand => ({
  name: "clip.slip",
  args: { item: item as ItemId, delta, clamp: true },
});

const slideCommand = (item: string, delta: number): MachineCommand => ({
  name: "clip.slide",
  args: { item: item as ItemId, delta, clamp: true },
});

interface Placement {
  readonly command: MachineCommand | null;
  readonly guide: SnapGuide | null;
  readonly preview: DropPreview | null;
}

/** Recomputes the open gesture's update for the pointer, with its guide, ghost and cursor. */
function evaluate(st: Step): void {
  const m = st.s.mode;
  const p = st.s.pointer;
  if (p === null || !("origin" in m)) return;
  const hit = st.deps.hitTest(p.x, p.y);
  const dt = hit.time - m.origin.time;
  const snapshot = m.origin.snapshot;
  let place: Placement;
  let next: GestureMode = m;
  switch (m.name) {
    case "moving": {
      const r = placeMove(st, m, hit, p);
      place = r;
      next = { ...m, toTrack: r.toTrack };
      break;
    }
    case "trimming":
    case "ripple_trimming": {
      const s = snapAt(st, snapshot, m.origin.exclude, m.edge0 + dt, [0]);
      const to = Math.max(0, s.t);
      place = { command: trimCommand(m.name, m, to), guide: s.target, preview: null };
      break;
    }
    case "rolling": {
      const s = snapAt(st, snapshot, m.origin.exclude, m.cut0 + dt, [0]);
      place = {
        command: rollCommand(m.left, m.right, Math.max(0, s.t)),
        guide: s.target,
        preview: null,
      };
      break;
    }
    case "slipping": {
      // Drag the media under the clip: dragging right brings earlier material in (Premiere).
      const delta = snapToFrame(-dt, snapshot.fps) + 0;
      place = { command: slipCommand(m.item, delta), guide: null, preview: null };
      break;
    }
    case "sliding": {
      const s = snapAt(st, snapshot, m.origin.exclude, m.start0 + dt, [0, m.duration]);
      const at = Math.max(0, s.t);
      place = { command: slideCommand(m.item, at - m.start0), guide: s.target, preview: null };
      break;
    }
    case "keyframe_dragging": {
      const s = snapAt(st, snapshot, null, m.keyframeTime + dt, [0]);
      const t = Math.max(0, s.t);
      place = {
        command: st.deps.keyframes?.command(m.item, m.keyframeTime, t) ?? null,
        guide: s.target,
        preview: null,
      };
      break;
    }
  }
  const tx = withPending(next.tx, place.command);
  const scroll = next.name === "slipping" ? null : autoScroll(st, p, next.name === "moving");
  st.set({ ...next, tx, scroll });
  st.guides(place.guide === null || tx.clamped ? [] : [place.guide]);
  st.preview(place.preview);
  st.cursor(gestureCursor({ ...next, tx }));
}

function withPending(tx: TxState, command: MachineCommand | null): TxState {
  const pendingKey = keyOf(command);
  if (pendingKey === tx.pendingKey) return tx;
  return { ...tx, pending: command, pendingKey };
}

function placeMove(
  st: Step,
  m: MovingMode,
  hit: HitResult,
  p: { x: number; y: number },
): Placement & { toTrack: string } {
  const snapshot = m.origin.snapshot;
  const dx = p.x - m.origin.x;
  const dy = p.y - m.origin.y;
  // Shift constrains the drag to its main axis: time only, or track only.
  const axis = st.s.keys.shift ? (Math.abs(dx) >= Math.abs(dy) ? "x" : "y") : null;
  const raw = Math.max(m.minStart, m.start0 + (axis === "y" ? 0 : hit.time - m.origin.time));
  let toTrack = m.toTrack;
  if (m.trackChange) {
    if (axis === "x") toTrack = m.homeTrack;
    else {
      const over = trackOf(snapshot, hit.trackId);
      const home = trackOf(snapshot, m.homeTrack);
      if (
        over !== undefined &&
        home !== undefined &&
        over.kind === home.kind &&
        over.locked !== true
      ) {
        toTrack = over.id;
      }
    }
  }
  if (m.copy) {
    if (isMain(snapshot, m.homeTrack)) {
      // Copies of main-track clips go right after the last copied one (spec §4.8).
      return {
        toTrack,
        command: { name: "item.duplicate", args: { items: m.items as ItemId[] } },
        guide: null,
        preview: null,
      };
    }
    const s =
      axis === "y" ? { t: m.start0, target: null } : snapAt(st, snapshot, null, raw, m.offsets);
    const at = Math.max(m.minStart, s.t);
    const end = at + m.duration;
    return {
      toTrack,
      command: {
        name: "item.duplicate",
        args: { items: m.items as ItemId[], offset: at - m.start0, mode: "new_track" },
      },
      guide: s.target,
      preview: {
        trackId: m.homeTrack,
        start: at,
        end,
        mode: overlaps(snapshot, m.homeTrack, at, end, null) ? "new_track" : "place",
      },
    };
  }
  if (isMain(snapshot, toTrack)) {
    // CapCut-style: on the main track the clip goes in at the nearest cut (spec D8, D9).
    const at = nearestCut(liftedCuts(snapshot, toTrack, m.origin.exclude), raw);
    return {
      toTrack,
      command: moveCommand(m, at, toTrack, true),
      guide: { kind: "edge", t: at },
      preview: { trackId: toTrack, start: at, end: at + m.duration, mode: "insert" },
    };
  }
  const s =
    axis === "y"
      ? { t: m.start0, target: null }
      : snapAt(st, snapshot, m.origin.exclude, raw, m.offsets);
  const at = Math.max(m.minStart, s.t);
  const end = at + m.duration;
  return {
    toTrack,
    command: moveCommand(m, at, toTrack, false),
    guide: s.target,
    preview: {
      trackId: toTrack,
      start: at,
      end,
      mode: overlaps(snapshot, toTrack, at, end, m.origin.exclude) ? "new_track" : "place",
    },
  };
}

/** The auto-scroll step near a lane edge (faster the deeper), or null away from them. */
function autoScroll(
  st: Step,
  p: { x: number; y: number },
  vertical: boolean,
): { dx: number; dy: number } | null {
  const l = st.deps.lanes();
  const { env } = st.s;
  const step = (depth: number): number =>
    Math.min(AUTO_SCROLL_MAX_PX, Math.ceil((AUTO_SCROLL_MAX_PX * depth) / AUTO_SCROLL_EDGE_PX));
  let dx = 0;
  let dy = 0;
  if (p.x < l.left + AUTO_SCROLL_EDGE_PX) {
    if (env.scrollX > 0) dx = -step(l.left + AUTO_SCROLL_EDGE_PX - p.x);
  } else if (p.x > l.right - AUTO_SCROLL_EDGE_PX) dx = step(p.x - (l.right - AUTO_SCROLL_EDGE_PX));
  if (vertical) {
    if (p.y < l.top + AUTO_SCROLL_EDGE_PX) {
      if (env.scrollY > 0) dy = -step(l.top + AUTO_SCROLL_EDGE_PX - p.y);
    } else if (p.y > l.bottom - AUTO_SCROLL_EDGE_PX)
      dy = step(p.y - (l.bottom - AUTO_SCROLL_EDGE_PX));
  }
  return dx === 0 && dy === 0 ? null : { dx, dy };
}

/** Sends the pending update if it differs from the last one sent (never the same one twice). */
function flush(st: Step): void {
  const m = st.s.mode;
  if (!("tx" in m)) return;
  const tx = m.tx;
  if (tx.pending === null || tx.pendingKey === tx.sentKey) return;
  st.emit({ type: "update_tx", gesture: tx.gesture, command: tx.pending });
  st.set({ ...m, tx: { ...tx, sentKey: tx.pendingKey, sent: true } });
}

/** Release: the last update, then commit (or cancel when nothing ever went out). */
function finishGesture(st: Step, label: string): void {
  flush(st);
  const m = st.s.mode;
  if (!("tx" in m)) return;
  if (m.tx.sent) st.emit({ type: "commit_tx", gesture: m.tx.gesture });
  else st.emit({ type: "cancel_tx", gesture: m.tx.gesture, reason: "unchanged" });
  st.guides([]);
  if (m.name === "dropping") toIdle(st, label);
  else toHover(st, label);
}

/** The engine ended the gesture, or its target is gone: close the pairing and go idle. */
function abort(st: Step, reason: CancelReason): void {
  const m = st.s.mode;
  if (m.name === "pressed") {
    toIdle(st, "aborted");
    return;
  }
  if (!("tx" in m)) return;
  st.emit({ type: "cancel_tx", gesture: m.tx.gesture, reason });
  toIdle(st, "aborted");
}

function onFrame(st: Step): void {
  const m = st.s.mode;
  if (m.name === "scrubbing") {
    if (m.pending === null) return;
    st.emit({ type: "seek", t: m.pending });
    st.env({ playhead: m.pending });
    st.set({ ...m, sent: m.pending, pending: null });
    return;
  }
  if (m.name === "dropping") {
    flush(st);
    return;
  }
  if (!("origin" in m)) return;
  // The host scrolled after the last frame: what is under the pointer changed.
  if (m.scroll !== null) evaluate(st);
  flush(st);
  const after = st.s.mode;
  if ("scroll" in after && after.scroll !== null) {
    const { dx, dy } = after.scroll;
    const { env } = st.s;
    st.env({
      scrollX: Math.max(0, env.scrollX + Math.round((dx * FLICKS_PER_SECOND) / env.pxPerSec)),
      scrollY: Math.max(0, env.scrollY + dy),
    });
    st.emit({ type: "scroll_by", dx, dy });
  }
}

function onUp(st: Step, e: PointerPoint): void {
  st.keys(keysOf(e));
  st.pointer(e.x, e.y);
  const m = st.s.mode;
  switch (m.name) {
    case "idle":
    case "hover":
      return;
    case "pressed":
      if (m.collapse !== null && !m.dragged) st.select(m.collapse);
      toHover(st, "pointer.up");
      return;
    case "selecting": {
      if (m.dragging || travelled(m, e)) {
        const rect = rectBetween(m, e);
        const { env, keys } = st.s;
        st.select(
          marqueeSelection(env.model, st.deps.itemsInRect(rect), {
            alt: keys.alt,
            linked: env.linkedSelection,
            additive: m.additive,
            current: env.selection,
          }),
        );
      } else if (!m.additive) st.select([]);
      toHover(st, "pointer.up");
      return;
    }
    case "scrubbing": {
      const s = scrubAt(st, st.deps.hitTest(e.x, e.y).time);
      if (s.t !== m.sent) {
        st.emit({ type: "seek", t: s.t });
        st.env({ playhead: s.t });
      }
      toHover(st, "pointer.up");
      return;
    }
    case "panning":
      toHover(st, "pointer.up");
      return;
    case "zooming": {
      const t = st.deps.hitTest(e.x, e.y).time;
      if (m.dragging || travelled(m, e)) {
        const t0 = Math.max(0, Math.min(m.time, t));
        const t1 = Math.max(m.time, t);
        if (t1 > t0) st.emit({ type: "zoom_to_range", t0, t1 });
      } else {
        const factor = st.s.keys.alt ? 1 / ZOOM_CLICK_FACTOR : ZOOM_CLICK_FACTOR;
        st.emit({ type: "zoom_by", factor, anchorX: e.x });
      }
      toHover(st, "pointer.up");
      return;
    }
    case "razor": {
      const r = razorAt(st, st.deps.hitTest(e.x, e.y));
      const items = razorCuts(st, r.item, r.at);
      if (items.length > 0) {
        st.emit({
          type: "command",
          command: { name: "clip.split", args: { items: items as ItemId[], at: r.at } },
        });
      }
      toHover(st, "pointer.up");
      return;
    }
    case "dropping":
      endDrop(st, "media_leave", "cancel");
      return;
    default:
      evaluate(st);
      finishGesture(st, "pointer.up");
  }
}

const inside = (it: { start: number; duration: number }, t: number): boolean =>
  it.start < t && t < it.start + it.duration;

/** What a razor click at `at` splits: the clip under it, or with Shift every clip there. */
function razorCuts(st: Step, item: string | null, at: number): readonly string[] {
  const model = st.s.env.model;
  if (st.s.keys.shift) {
    const hits: string[] = [];
    for (const track of model.tracks) {
      if (track.locked === true) continue;
      for (const it of itemsOn(model, track.id)) if (inside(it, at)) hits.push(it.id);
    }
    return editTargets(model, hits).items;
  }
  const it = itemOf(model, item);
  if (it === undefined || isLocked(model, it.trackId) || !inside(it, at)) return [];
  return [it.id];
}

// ---- cancel -------------------------------------------------------------------------------------

/** Esc, a lost pointer or a lost window: everything in progress rolls back. */
function cancel(st: Step, reason: CancelReason): void {
  const m = st.s.mode;
  switch (m.name) {
    case "idle":
      return;
    case "hover":
      if (reason !== "escape") toIdle(st, "cancel");
      return;
    case "scrubbing":
      if (m.sent !== m.from) st.emit({ type: "seek", t: m.from });
      st.env({ playhead: m.from });
      break;
    case "panning":
      if (st.s.env.scrollX !== m.scrollX0 || st.s.env.scrollY !== m.scrollY0) {
        st.emit({ type: "scroll_to", scrollX: m.scrollX0, scrollY: m.scrollY0 });
        st.env({ scrollX: m.scrollX0, scrollY: m.scrollY0 });
      }
      break;
    default:
      if ("tx" in m) st.emit({ type: "cancel_tx", gesture: m.tx.gesture, reason });
  }
  toIdle(st, "cancel");
}

// ---- keys and actions ---------------------------------------------------------------------------

function onKey(
  st: Step,
  e: Modifiers & { readonly key: string; readonly action?: MachineAction | null },
  down: boolean,
): void {
  st.keys(keysAfter(e, down));
  if (MODIFIER_KEYS.has(e.key)) {
    refresh(st);
    return;
  }
  if (!down) return;
  const action = e.action !== undefined ? e.action : machineKeyAction(e.key, e);
  if (action !== null) act(st, action);
}

function act(st: Step, action: MachineAction): void {
  if (action === "cancel") {
    cancel(st, "escape");
    return;
  }
  if (isMachineBusy(st.s)) return; // S, Delete and the tools wait for the gesture to end
  const { env } = st.s;
  const model = env.model;
  switch (action) {
    case "split":
      split(st);
      return;
    case "delete":
    case "ripple_delete": {
      const t = editTargets(model, env.selection);
      if (t.partial) {
        st.notice("partial_group");
        return;
      }
      if (t.items.length === 0) return;
      const items = t.items as ItemId[];
      st.emit({
        type: "command",
        command:
          action === "delete"
            ? { name: "item.delete", args: { items } }
            : { name: "timeline.ripple_delete", args: { items } },
      });
      return;
    }
    case "nudge_left":
    case "nudge_right":
    case "nudge_left_10":
    case "nudge_right_10":
      nudge(st, action);
      return;
    case "toggle_snapping":
      st.env({ snapping: !env.snapping });
      st.emit({ type: "set_snapping", on: !env.snapping });
      refresh(st);
      return;
    case "select_all":
      st.select(model.items.map((it) => it.id));
      return;
    case "deselect":
      st.select([]);
      return;
    default: {
      const tool = action.slice("tool.".length) as Tool;
      if (tool === env.tool) return;
      st.env({ tool });
      st.emit({ type: "set_tool", tool });
      refresh(st);
    }
  }
}

/**
 * Split at the playhead: every selected clip that strictly contains it (one per linked group;
 * the engine splits partners); with nothing selected, the clip under the playhead on the topmost
 * unlocked track that has one.
 */
function split(st: Step): void {
  const { env } = st.s;
  const model = env.model;
  const at = snapToFrame(env.playhead, model.fps);
  let items: readonly string[];
  if (env.selection.length > 0) {
    const t = editTargets(model, env.selection);
    if (t.partial) {
      st.notice("partial_group");
      return;
    }
    items = t.items.filter((id) => {
      const it = itemOf(model, id);
      return it !== undefined && inside(it, at) && !isLocked(model, it.trackId);
    });
  } else {
    items = [];
    for (const track of model.tracks) {
      if (track.locked === true) continue;
      const hit = itemsOn(model, track.id).find((it) => inside(it, at));
      if (hit !== undefined) {
        items = [hit.id];
        break;
      }
    }
  }
  if (items.length === 0) return;
  st.emit({
    type: "command",
    command: { name: "clip.split", args: { items: items as ItemId[], at } },
  });
}

/** `,` / `.`: move the selection one frame (Shift: ten). Main-track clips have no room to nudge. */
function nudge(
  st: Step,
  action: "nudge_left" | "nudge_right" | "nudge_left_10" | "nudge_right_10",
): void {
  const { env } = st.s;
  const model = env.model;
  const t = editTargets(model, env.selection);
  const items = t.items.filter((id) => {
    const it = itemOf(model, id);
    return it !== undefined && !isMain(model, it.trackId);
  });
  if (items.length === 0) return;
  const frames = action.endsWith("_10") ? NUDGE_FAR_FRAMES : 1;
  const delta =
    (action.startsWith("nudge_left") ? -frames : frames) * frameDurationFlicks(model.fps);
  const earliest = Math.min(...items.map((id) => itemOf(model, id)?.start ?? 0));
  if (earliest + delta < 0) return;
  st.emit({
    type: "command",
    command: {
      name: "item.move",
      args: { items: items as ItemId[], delta, ...(t.partial && { unlink: true }) },
    },
  });
}

// ---- dropping media -----------------------------------------------------------------------------

function onMediaOver(st: Step, e: PointerPoint & { readonly media: DroppedMedia }): void {
  st.keys(keysOf(e));
  st.pointer(e.x, e.y);
  const m = st.s.mode;
  if (m.name === "dropping") {
    placeDrop(st, e.media);
    return;
  }
  if (isMachineBusy(st.s)) return;
  const model = st.s.env.model;
  if (dropTrack(model, st.deps.hitTest(e.x, e.y).trackId, e.media) === null) return;
  const gesture = st.s.lastGesture + 1;
  st.s = { ...st.s, lastGesture: gesture };
  st.emit({ type: "begin_tx", gesture, edit: "insert" });
  st.go(
    { name: "dropping", tx: newTx(gesture, "insert", null), media: e.media, snapshot: model },
    "media.over",
  );
  placeDrop(st, e.media);
}

/** Where the card lands now: the `timeline.insert` update, its guide and ghost (spec INV-40). */
function placeDrop(st: Step, media: DroppedMedia): void {
  const m = st.s.mode;
  const p = st.s.pointer;
  if (m.name !== "dropping" || p === null) return;
  const { env } = st.s;
  const snapshot = m.snapshot;
  const hit = st.deps.hitTest(p.x, p.y);
  const hovered = trackOf(snapshot, hit.trackId) === undefined ? null : hit.trackId;
  const track = dropTrack(snapshot, hovered, media);
  if (track === null) {
    endDrop(st, "media_leave", "media.leave");
    return;
  }
  const duration = media.duration ?? STILL_DROP_DURATION;
  const raw = Math.max(0, hit.time);
  let at: number;
  let mode: "insert" | "overwrite" | "new_track" | undefined;
  let guide: SnapGuide | null;
  if (isMain(snapshot, track)) {
    at = nearestCut(liftedCuts(snapshot, track, null), raw);
    mode = "insert";
    guide = { kind: "edge", t: at };
  } else {
    const s = snapAt(st, snapshot, null, raw, [0, duration]);
    at = Math.max(0, s.t);
    guide = s.target;
    const under = hovered === track ? itemAt(snapshot, track, raw) : undefined;
    // Onto a clip: the Editing preference; onto free space that is too short: a new track.
    if (under !== undefined) mode = env.dropOnClip;
    else if (overlaps(snapshot, track, at, at + duration, null)) mode = "new_track";
  }
  const command: MachineCommand = {
    name: "timeline.insert",
    args: {
      sequence: env.sequence as SequenceId,
      track: track as TrackId,
      media: media.id as MediaId,
      at,
      ...(mode !== undefined && { mode }),
    },
  };
  const tx = withPending(m.tx, command);
  st.set({ ...m, media, tx });
  st.guides(guide === null ? [] : [guide]);
  st.preview({ trackId: track, start: at, end: at + duration, mode: mode ?? "place" });
  st.cursor(gestureCursor({ ...m, tx }));
}

function onMediaDrop(st: Step, e: PointerPoint & { readonly media: DroppedMedia }): void {
  if (st.s.mode.name !== "dropping") onMediaOver(st, e);
  else {
    st.keys(keysOf(e));
    st.pointer(e.x, e.y);
    placeDrop(st, e.media);
  }
  if (st.s.mode.name !== "dropping") return;
  finishGesture(st, "media.drop");
}

function endDrop(st: Step, reason: CancelReason, label: string): void {
  const m = st.s.mode;
  if (m.name !== "dropping") return;
  st.emit({ type: "cancel_tx", gesture: m.tx.gesture, reason });
  toIdle(st, label);
}

// ---- host and engine feedback -------------------------------------------------------------------

/** The items a gesture edits: if one disappears (an agent deleted it), the gesture ends. */
function targetsOf(m: MachineMode): readonly string[] {
  switch (m.name) {
    case "pressed":
      return m.target.kind === "roll" ? [m.target.left, m.target.right] : [m.target.item];
    case "moving":
      return m.items;
    case "trimming":
    case "ripple_trimming":
    case "slipping":
    case "sliding":
    case "keyframe_dragging":
      return [m.item];
    case "rolling":
      return [m.left, m.right];
    default:
      return [];
  }
}

function onEnv(st: Step, patch: Partial<MachineState["env"]>): void {
  const before = st.s.env;
  st.env(patch);
  const m = st.s.mode;
  if (patch.model !== undefined && patch.model !== before.model) {
    const model = patch.model;
    if (targetsOf(m).some((id) => itemOf(model, id) === undefined)) {
      abort(st, "target_gone");
      return;
    }
  }
  if (m.name === "hover") hoverUpdate(st);
}

function onResult(st: Step, e: Extract<MachineEvent, { type: "engine.result" }>): void {
  const m = st.s.mode;
  if (!("tx" in m) || m.tx.gesture !== e.gesture) return;
  let tx: TxState;
  if (e.ok) {
    tx = { ...m.tx, refusal: null, clamped: e.clamped === true };
    if (tx.clamped) st.guides([]);
  } else if (
    e.code === "invalid" ||
    e.code === "locked" ||
    (e.code === "conflict" && e.reason === "item_in_gesture")
  ) {
    // A later position may work: keep the gesture (the engine kept the last good update).
    tx = {
      ...m.tx,
      refusal: {
        code: e.code,
        ...(e.reason !== undefined && { reason: e.reason }),
        ...(e.holder !== undefined && { holder: e.holder }),
      },
    };
  } else {
    abort(st, "engine");
    return;
  }
  const next = { ...m, tx } as GestureMode | DroppingMode;
  st.set(next);
  st.cursor(gestureCursor(next));
}
