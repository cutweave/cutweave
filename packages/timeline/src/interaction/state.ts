// SPDX-License-Identifier: GPL-3.0-or-later
// The machine's states (WP-2A.08). The state chart in `packages/timeline/README.md` names exactly
// these; the coverage test holds the two together.

import type { Rect, SnapGuide, TimelineModel } from "../model.ts";
import { DEFAULT_SNAP_PX } from "./constants.ts";
import type {
  Cursor,
  DropPreview,
  DroppedMedia,
  EngineRefusal,
  GestureEdit,
  HitZone,
  MachineCommand,
  MachineEnv,
} from "./types.ts";

/** Every state of the chart. */
export const MACHINE_STATES = [
  "idle",
  "hover",
  "pressed",
  "selecting",
  "moving",
  "trimming",
  "ripple_trimming",
  "rolling",
  "slipping",
  "sliding",
  "razor",
  "scrubbing",
  "keyframe_dragging",
  "panning",
  "zooming",
  "dropping",
] as const;

export type StateName = (typeof MACHINE_STATES)[number];

/** The states that hold an open gesture transaction. */
export const GESTURE_STATES: ReadonlySet<StateName> = new Set<StateName>([
  "moving",
  "trimming",
  "ripple_trimming",
  "rolling",
  "slipping",
  "sliding",
  "keyframe_dragging",
  "dropping",
]);

/** The pointer is outside the timeline (or nothing has happened yet). */
export interface IdleMode {
  readonly name: "idle";
}

/** The pointer is over the timeline, no button down. */
export interface HoverMode {
  readonly name: "hover";
  readonly zone: HitZone;
  readonly item: string | null;
}

/** What a press on a clip becomes once the pointer travels. */
export type PressTarget =
  /** Select tool on a clip body: a move, or a duplicate with Alt held when the drag starts. */
  | { readonly kind: "body"; readonly item: string }
  | {
      readonly kind: "edge";
      readonly item: string;
      readonly edge: "start" | "end";
      readonly gesture: "trim" | "ripple_trim";
      /** Edit only this member of its linked group (Alt at the press, or a partial selection). */
      readonly unlink: boolean;
    }
  | { readonly kind: "roll"; readonly left: string; readonly right: string }
  | { readonly kind: "slip"; readonly item: string }
  | { readonly kind: "slide"; readonly item: string }
  | {
      readonly kind: "keyframe";
      readonly item: string;
      readonly keyframeTime: number;
      /** The lane's property; null for the clip's own key row. */
      readonly prop: string | null;
    }
  /** A click only: nothing to drag (locked track, a still under the slip tool, ...). */
  | { readonly kind: "none"; readonly item: string; readonly notice: "partial_group" | null };

/** A button is down on a clip; under the drag threshold it is still a click. */
export interface PressedMode {
  readonly name: "pressed";
  readonly x: number;
  readonly y: number;
  /** Timeline time under the press, flicks. */
  readonly time: number;
  readonly target: PressTarget;
  /** The selection a click (release without travel) leaves, when it differs from the current. */
  readonly collapse: readonly string[] | null;
  /**
   * The pointer travelled although nothing can be dragged (`target.kind === "none"`): the notice
   * went out once, and the release is no click.
   */
  readonly dragged: boolean;
}

/** A gesture transaction and the engine traffic of its updates. */
export interface TxState {
  readonly gesture: number;
  readonly edit: GestureEdit;
  /** The update for the pointer's current position (not sent yet when its key differs). */
  readonly pending: MachineCommand | null;
  readonly pendingKey: string | null;
  /** Key of the last update sent (before the first: the key of "nothing changed", if any). */
  readonly sentKey: string | null;
  /** Key of the update that changes nothing (the gesture's starting position), if it has one. */
  readonly noopKey: string | null;
  /** An update went out. */
  readonly sent: boolean;
  /** The engine refused the latest update (it keeps the previous one, spec INV-302). */
  readonly refusal: EngineRefusal | null;
  /** The engine clamped the latest update. */
  readonly clamped: boolean;
}

/** Where a drag started and what it reads while it runs. */
export interface DragOrigin {
  readonly x: number;
  readonly y: number;
  readonly time: number;
  /** The document when the drag started: positions and snap candidates come from it. */
  readonly snapshot: TimelineModel;
  /** Items that move with the gesture (not snap candidates); `null` for none. */
  readonly exclude: ReadonlySet<string> | null;
}

interface DragFields {
  readonly tx: TxState;
  readonly origin: DragOrigin;
  /** Auto-scroll step per frame (CSS px) while the pointer is near a lane edge. */
  readonly scroll: { readonly dx: number; readonly dy: number } | null;
}

export interface MovingMode extends DragFields {
  readonly name: "moving";
  /** `item.duplicate` instead of `item.move` (Alt held when the drag started). */
  readonly copy: boolean;
  /** The given items (the primary first), as the command names them. */
  readonly items: readonly string[];
  readonly unlink: boolean;
  readonly primary: string;
  readonly homeTrack: string;
  readonly start0: number;
  readonly duration: number;
  /** Lowest start the primary may take (keeps every mover at or after 0). */
  readonly minStart: number;
  /** Snap probes: every edge of every mover, relative to the primary's start. */
  readonly offsets: readonly number[];
  /** The movers may change track (all given items are on the primary's track). */
  readonly trackChange: boolean;
  readonly toTrack: string;
}

export interface TrimmingMode extends DragFields {
  readonly name: "trimming" | "ripple_trimming";
  readonly item: string;
  readonly edge: "start" | "end";
  readonly edge0: number;
  readonly unlink: boolean;
}

export interface RollingMode extends DragFields {
  readonly name: "rolling";
  readonly left: string;
  readonly right: string;
  readonly cut0: number;
}

export interface SlippingMode extends DragFields {
  readonly name: "slipping";
  readonly item: string;
}

export interface SlidingMode extends DragFields {
  readonly name: "sliding";
  readonly item: string;
  readonly start0: number;
  readonly duration: number;
}

export interface KeyframeMode extends DragFields {
  readonly name: "keyframe_dragging";
  readonly item: string;
  readonly keyframeTime: number;
  readonly prop: string | null;
}

/** A bin card over the lanes: a `timeline.insert` gesture, the last update kept on drop. */
export interface DroppingMode {
  readonly name: "dropping";
  readonly tx: TxState;
  readonly media: DroppedMedia;
  readonly snapshot: TimelineModel;
}

export interface SelectingMode {
  readonly name: "selecting";
  readonly x: number;
  readonly y: number;
  /** Shift: add to the selection. */
  readonly additive: boolean;
  /** The marquee is out (the pointer travelled). */
  readonly dragging: boolean;
}

export interface ScrubbingMode {
  readonly name: "scrubbing";
  /** The playhead before the scrub (Esc goes back to it). */
  readonly from: number;
  /** The last seek sent, and a newer one waiting for the frame. */
  readonly sent: number;
  readonly pending: number | null;
}

export interface PanningMode {
  readonly name: "panning";
  readonly x: number;
  readonly y: number;
  readonly scrollX0: number;
  readonly scrollY0: number;
}

export interface ZoomingMode {
  readonly name: "zooming";
  readonly x: number;
  readonly y: number;
  readonly time: number;
  readonly dragging: boolean;
}

export interface RazorMode {
  readonly name: "razor";
  /** The clip under the blade, and the snapped cut time. */
  readonly item: string | null;
  readonly at: number;
}

export type GestureMode =
  | MovingMode
  | TrimmingMode
  | RollingMode
  | SlippingMode
  | SlidingMode
  | KeyframeMode;

export type MachineMode =
  | IdleMode
  | HoverMode
  | PressedMode
  | SelectingMode
  | GestureMode
  | DroppingMode
  | ScrubbingMode
  | PanningMode
  | ZoomingMode
  | RazorMode;

/** The modifiers, all known. */
export interface HeldKeys {
  readonly shift: boolean;
  readonly alt: boolean;
  readonly ctrl: boolean;
  readonly meta: boolean;
}

export interface MachineState {
  readonly mode: MachineMode;
  readonly env: MachineEnv;
  /** The last pointer position over the canvas (modifier keys and frames re-read it). */
  readonly pointer: { readonly x: number; readonly y: number } | null;
  readonly keys: HeldKeys;
  /** What the host was last told, so only changes go out. */
  readonly cursor: Cursor;
  readonly guides: readonly SnapGuide[];
  readonly preview: DropPreview | null;
  readonly marquee: Rect | null;
  /** The last gesture id handed out. */
  readonly lastGesture: number;
}

export const DEFAULT_MACHINE_ENV: MachineEnv = {
  model: { fps: { num: 25, den: 1 }, tracks: [], items: [] },
  sequence: "",
  tool: "select",
  pxPerSec: 100,
  scrollX: 0,
  scrollY: 0,
  playhead: 0,
  selection: [],
  snapping: true,
  snapThresholdPx: DEFAULT_SNAP_PX,
  snapKinds: ["playhead", "edge", "marker", "beat", "in_out"],
  linkedSelection: true,
  dropOnClip: "overwrite",
  nudgeFrames: 1,
  inOut: null,
  heldBy: new Map(),
};

export const NO_KEYS: HeldKeys = { shift: false, alt: false, ctrl: false, meta: false };

export function initialMachineState(env: Partial<MachineEnv> = {}): MachineState {
  return {
    mode: { name: "idle" },
    env: { ...DEFAULT_MACHINE_ENV, ...env },
    pointer: null,
    keys: NO_KEYS,
    cursor: "default",
    guides: [],
    preview: null,
    marquee: null,
    lastGesture: 0,
  };
}

/** The open gesture's transaction, if the state has one. */
export function openTx(state: MachineState): TxState | null {
  const m = state.mode;
  return "tx" in m ? m.tx : null;
}

/** A gesture or another press is in progress: one-shot keys (S, Delete) wait. */
export function isMachineBusy(state: MachineState): boolean {
  return state.mode.name !== "idle" && state.mode.name !== "hover";
}
