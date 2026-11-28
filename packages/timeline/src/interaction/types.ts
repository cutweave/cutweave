// SPDX-License-Identifier: GPL-3.0-or-later
// Types of the timeline interaction state machine (WP-2A.08, arch §16, timeline spec §5-§7).
// Everything here is plain data: events in, intents out, a state in between. Nothing refers to
// the DOM, React or the renderer; the host injects hit-testing and snapping (`MachineDeps`).
import type {
  ClipRippleTrim,
  ClipRoll,
  ClipSlide,
  ClipSlip,
  ClipSplit,
  ClipTrim,
  ItemDelete,
  ItemDuplicate,
  ItemMove,
  KeyframeMove,
  TimelineInsert,
  TimelineRippleDelete,
} from "@cutweave/protocol";
import type { SnapKindName } from "@cutweave/protocol/wasm";
import type { HitResult, HitZone } from "../hit-test.ts";
import type { Rect, SnapGuide, TimelineModel } from "../model.ts";

// ---- tools, modifiers, cursors ----------------------------------------------------------------

/**
 * The toolbar's tool modes. `select` moves, trims (edges) and marquee-selects; `ripple` and
 * `roll` work on edges; `slip` and `slide` on clip bodies; `razor` splits; `hand` pans; `zoom`
 * zooms (click in, Alt+click out, drag a range).
 */
export type Tool = "select" | "ripple" | "roll" | "slip" | "slide" | "razor" | "hand" | "zoom";

export const TOOLS: readonly Tool[] = [
  "select",
  "ripple",
  "roll",
  "slip",
  "slide",
  "razor",
  "hand",
  "zoom",
];

/** Modifier keys as the event reported them (absent: not held). */
export interface Modifiers {
  readonly shift?: boolean;
  readonly alt?: boolean;
  readonly ctrl?: boolean;
  readonly meta?: boolean;
}

/** What the pointer shows; the host maps each to a CSS cursor. */
export type Cursor =
  /** Not the machine's (track headers): keep the view's own cursor. */
  | "auto"
  | "default"
  | "move"
  | "copy"
  | "trim-start"
  | "trim-end"
  | "ripple-start"
  | "ripple-end"
  | "roll"
  | "slip"
  | "slide"
  | "razor"
  | "grab"
  | "grabbing"
  | "zoom-in"
  | "zoom-out"
  | "scrub"
  /** The edit is not possible here (locked track, refused by the engine). */
  | "not-allowed"
  /** Another actor's gesture holds an item this edit touches (`Conflict{item_in_gesture}`). */
  | "locked";

// ---- environment ------------------------------------------------------------------------------

/** What a dragged bin card carries (WP-1M.03). */
export interface DroppedMedia {
  readonly id: string;
  readonly kind: "video" | "audio" | "image";
  /** Length in flicks; `null` for stills (the engine gives them 5 s). */
  readonly duration: number | null;
}

/**
 * Host state the machine reads. The host keeps it current with `env` events; the machine also
 * updates `selection`, `tool` and `snapping` itself when it emits the matching intents.
 */
export interface MachineEnv {
  /** The read-only timeline projection of the active sequence (`docStore`). */
  readonly model: TimelineModel;
  /** The active sequence id (`timeline.insert` names it). */
  readonly sequence: string;
  readonly tool: Tool;
  /** Zoom, CSS pixels per second. */
  readonly pxPerSec: number;
  /** Horizontal scroll (flicks at the lane's left edge) and vertical scroll (CSS px). */
  readonly scrollX: number;
  readonly scrollY: number;
  /** Playhead, flicks. */
  readonly playhead: number;
  /** Selected item ids, in selection order (`viewStore.selection`, items only). */
  readonly selection: readonly string[];
  /** The snap toggle. The frame grid always applies (spec §6). */
  readonly snapping: boolean;
  /** Snap distance in CSS px (Timeline preference, default 8). */
  readonly snapThresholdPx: number;
  /** Snap kinds the preferences enable (default all). */
  readonly snapKinds: readonly SnapKindName[];
  /** Linked selection (the toolbar's link toggle): a click selects the whole linked group. */
  readonly linkedSelection: boolean;
  /** Editing preference: media dropped onto a clip outside the main track (WP-2A.13). */
  readonly dropOnClip: "overwrite" | "insert";
  /** Editing preference: frames `,` and `.` move the selection (Shift: ten times it). */
  readonly nudgeFrames: number;
  /** Sequence in and out (snap candidates). */
  readonly inOut: { readonly in: number; readonly out: number } | null;
  /** Items another actor's gesture or proposal holds, to the actor (`locksStore.byItem`). */
  readonly heldBy: ReadonlyMap<string, string>;
}

// ---- events -----------------------------------------------------------------------------------

/** A pointer position in canvas CSS pixels, with the modifiers held. */
export interface PointerPoint extends Modifiers {
  readonly x: number;
  readonly y: number;
}

/** The actions a key, the toolbar or a menu can ask of the timeline. */
export type MachineAction =
  | "cancel"
  | "split"
  | "delete"
  | "ripple_delete"
  | "toggle_snapping"
  | "nudge_left"
  | "nudge_right"
  | "nudge_left_10"
  | "nudge_right_10"
  | "select_all"
  | "deselect"
  | `tool.${Tool}`;

/** Why the engine refused a gesture update or a transaction. */
export interface EngineRefusal {
  /** The machine code (`invalid`, `locked`, `conflict`, `not_found`, ...). */
  readonly code: string;
  /** `item_in_gesture` for another actor's soft lock. */
  readonly reason?: string;
  /** The actor holding the item. */
  readonly holder?: string;
}

export type MachineEvent =
  /** A button went down over the timeline canvas (0 primary, 1 middle, 2 secondary). */
  | ({ readonly type: "pointer.down"; readonly button?: number } & PointerPoint)
  | ({ readonly type: "pointer.move" } & PointerPoint)
  | ({ readonly type: "pointer.up"; readonly button?: number } & PointerPoint)
  /** The pointer left the canvas (no capture): only hovering ends. */
  | { readonly type: "pointer.leave" }
  /** The browser took the pointer away (`pointercancel`, lost capture): like Esc. */
  | { readonly type: "pointer.cancel" }
  /** The window lost focus: like Esc (the pointer-up may never arrive). */
  | { readonly type: "blur" }
  /**
   * A key went down in the timeline panel. `action` is what the host's keymap made of it
   * (WP-2H.05; `null`: nothing); absent, the machine's default table decides (`MACHINE_KEYS`).
   */
  | ({
      readonly type: "key.down";
      readonly key: string;
      readonly action?: MachineAction | null;
    } & Modifiers)
  | ({ readonly type: "key.up"; readonly key: string } & Modifiers)
  /** A toolbar button or a menu item (Split, Delete, a tool, the snap toggle). */
  | { readonly type: "action"; readonly action: MachineAction }
  /** A bin card is dragged over the canvas. */
  | ({ readonly type: "media.over"; readonly media: DroppedMedia } & PointerPoint)
  /** The card was released over the canvas. */
  | ({ readonly type: "media.drop"; readonly media: DroppedMedia } & PointerPoint)
  /** The card left the canvas, or the drag ended elsewhere. */
  | { readonly type: "media.leave" }
  /** An animation frame: coalesced engine traffic goes out, auto-scroll steps. */
  | { readonly type: "frame" }
  /** The host's state changed (document patch, view store, preferences, locks). */
  | { readonly type: "env"; readonly env: Partial<MachineEnv> }
  /** The engine's answer to the gesture's latest update (or to opening it). */
  | ({ readonly type: "engine.result"; readonly gesture: number } & (
      | { readonly ok: true; readonly clamped?: boolean }
      | ({ readonly ok: false } & EngineRefusal)
    ))
  /**
   * The engine closed the gesture on its own: `gesture.cancelled{reason:"idle_timeout"}`, the
   * connection dropped, or the transaction is gone. The document is already rolled back.
   */
  | { readonly type: "engine.cancelled"; readonly gesture: number; readonly reason: string };

// ---- intents ----------------------------------------------------------------------------------

/** The undo step a gesture becomes (the host labels it: "Move clip", "Ripple trim", ...). */
export type GestureEdit =
  | "move"
  | "duplicate"
  | "trim"
  | "ripple_trim"
  | "roll"
  | "slip"
  | "slide"
  | "keyframe"
  | "insert";

/** A command the machine asks for, with its parameters (timeline spec §3.9). */
export type MachineCommand =
  | { readonly name: "item.move"; readonly args: ItemMove }
  | { readonly name: "item.duplicate"; readonly args: ItemDuplicate }
  | { readonly name: "clip.trim"; readonly args: ClipTrim }
  | { readonly name: "clip.ripple_trim"; readonly args: ClipRippleTrim }
  | { readonly name: "clip.roll"; readonly args: ClipRoll }
  | { readonly name: "clip.slip"; readonly args: ClipSlip }
  | { readonly name: "clip.slide"; readonly args: ClipSlide }
  | { readonly name: "timeline.insert"; readonly args: TimelineInsert }
  | { readonly name: "keyframe.move"; readonly args: KeyframeMove }
  | { readonly name: "clip.split"; readonly args: ClipSplit }
  | { readonly name: "item.delete"; readonly args: ItemDelete }
  | { readonly name: "timeline.ripple_delete"; readonly args: TimelineRippleDelete };

/** Why a gesture's transaction was cancelled. */
export type CancelReason =
  /** Esc, or the Cancel action. */
  | "escape"
  /** The browser took the pointer away. */
  | "pointer_cancel"
  /** The window lost focus. */
  | "blur"
  /** Released before any update went out: there is nothing to keep. */
  | "unchanged"
  /** The dragged item disappeared (another actor deleted it, the document was reset). */
  | "target_gone"
  /** The media card left the timeline, or was dropped where nothing can land. */
  | "media_leave"
  /**
   * The engine already closed the transaction (`engine.cancelled`, or a refusal no later
   * position can fix). The host sends nothing; the intent only closes the pairing.
   */
  | "engine";

/** Where a dragged clip or a dropped card would land, for the drop ghost (WP-2A.10 draws it). */
export interface DropPreview {
  readonly trackId: string;
  readonly start: number;
  readonly end: number;
  /** `insert`: splits and ripples (the main track); `overwrite`; `new_track`: a track above. */
  readonly mode: "insert" | "overwrite" | "new_track" | "place";
}

/** Something the person should be told (not an engine error: those are the host's). */
export type NoticeCode =
  /** The selection holds only some members of a linked group and the edit cannot unlink. */
  "partial_group";

export type MachineIntent =
  /** Open a gesture transaction: `edit.begin_tx{kind:"gesture"}`. `gesture` is the machine's id. */
  | { readonly type: "begin_tx"; readonly gesture: number; readonly edit: GestureEdit }
  /**
   * A gesture update: the command with `_meta.tx` and `_meta.replace: true` (spec INV-301). At
   * most one per animation frame; the release flushes the last one before `commit_tx`.
   */
  | {
      readonly type: "update_tx";
      readonly gesture: number;
      readonly command: MachineCommand;
    }
  /** Release: `edit.commit_tx` (one undo step). */
  | { readonly type: "commit_tx"; readonly gesture: number }
  /** Roll back: `edit.cancel_tx` (nothing to send when `reason` is `engine`). */
  | { readonly type: "cancel_tx"; readonly gesture: number; readonly reason: CancelReason }
  /** A one-shot command (its own undo step): split, delete, ripple delete, nudge. */
  | { readonly type: "command"; readonly command: MachineCommand }
  /** Replace the item selection (`viewStore`). */
  | { readonly type: "set_selection"; readonly ids: readonly string[] }
  | { readonly type: "set_cursor"; readonly cursor: Cursor }
  | { readonly type: "set_tool"; readonly tool: Tool }
  | { readonly type: "set_snapping"; readonly on: boolean }
  /** Move the playhead (frame-aligned), as the transport's latest-wins seek. */
  | { readonly type: "seek"; readonly t: number }
  /** Scroll to an absolute position (panning): `scrollX` flicks, `scrollY` CSS px. */
  | { readonly type: "scroll_to"; readonly scrollX: number; readonly scrollY: number }
  /** Scroll by CSS px (auto-scroll at the edges during a drag). */
  | { readonly type: "scroll_by"; readonly dx: number; readonly dy: number }
  /** Zoom by `factor` keeping the time under canvas x `anchorX` (`TimelineView.zoomBy`). */
  | { readonly type: "zoom_by"; readonly factor: number; readonly anchorX: number }
  /** Show `[t0, t1]` across the lane (`TimelineView.zoomToRange`). */
  | { readonly type: "zoom_to_range"; readonly t0: number; readonly t1: number }
  /** Snap guides to draw (`viewState.snapGuides`; empty: none). */
  | { readonly type: "snap_guides"; readonly guides: readonly SnapGuide[] }
  /** The marquee (or zoom band) rectangle in canvas CSS px; `null` clears it. */
  | { readonly type: "marquee"; readonly rect: Rect | null }
  /** Where the dragged clip or card lands; `null` clears the ghost. */
  | { readonly type: "preview"; readonly preview: DropPreview | null }
  | { readonly type: "notice"; readonly code: NoticeCode };

export type IntentType = MachineIntent["type"];

// ---- dependencies -----------------------------------------------------------------------------

/** A snap query (spec §6): the candidates come from `model`, minus the `exclude`d movers. */
export interface SnapQuery {
  /** Whose item edges and markers are candidates (the gesture's snapshot while dragging). */
  readonly model: TimelineModel;
  /** Items that move with the gesture: not candidates. Pass the same set object while it holds. */
  readonly exclude: ReadonlySet<string> | null;
  /** The raw time, flicks. */
  readonly raw: number;
  /** The probes relative to `raw`, flicks: `[0]` for an edge, `[0, duration]` for a clip. */
  readonly offsets: readonly number[];
  readonly pxPerSec: number;
  readonly thresholdPx: number;
  readonly kinds: readonly SnapKindName[];
  /** Snapping off or Ctrl/Cmd held: the frame grid only. */
  readonly suspended: boolean;
  readonly playhead: number;
  readonly inOut: { readonly in: number; readonly out: number } | null;
}

export interface SnapOutcome {
  /** Frame-aligned. */
  readonly t: number;
  /** The candidate a probe landed on (draw a guide), or null. */
  readonly target: { readonly kind: SnapKindName; readonly t: number } | null;
}

/** Shared snapping (WP-2A.07). `createSnapPort()` is the wasm implementation. */
export interface SnapPort {
  snap(query: SnapQuery): SnapOutcome;
}

/** Keyframe drags (WP-2C.05 supplies it). Without one, a press on a keyframe acts on the clip. */
export interface KeyframeDragHandler {
  /**
   * The update that puts the key at `time` (sequence time, frame-aligned), or null for none.
   * `prop`: the property of the keyframe lane the key was pressed in; null for a diamond of the
   * clip's own key row (every property's key at that time).
   */
  command(
    item: string,
    keyframeTime: number,
    time: number,
    prop: string | null,
  ): MachineCommand | null;
}

/** Where animation frames come from (the canvas's own window; a 16 ms timer by default). */
export interface FrameScheduler {
  request(callback: () => void): unknown;
  cancel(handle: unknown): void;
}

/** What the machine needs from the host. Queries only: none of them may change the host. */
export interface MachineDeps {
  /** What is under canvas point `(x, y)` (`TimelineView.hitTest`). */
  hitTest(x: number, y: number): HitResult;
  /** Items whose clip box meets a canvas rect (`TimelineView.itemsInRect`). */
  itemsInRect(rect: Rect): readonly string[];
  /** The lane area in canvas CSS px (auto-scroll starts near its edges). */
  lanes(): {
    readonly left: number;
    readonly right: number;
    readonly top: number;
    readonly bottom: number;
  };
  readonly snap: SnapPort;
  readonly keyframes?: KeyframeDragHandler;
}

export type { HitResult, HitZone };
