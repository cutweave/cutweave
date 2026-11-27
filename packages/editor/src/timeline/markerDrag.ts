// SPDX-License-Identifier: GPL-3.0-or-later
// A sequence marker dragged along the ruler (issue #222). A press on a marker's pin belongs to
// this, not to the scrub: a click goes to the marker, a drag moves it. The drag is one gesture
// transaction of `timeline.move_marker` updates (the marker follows the pointer, one undo step),
// snapped as every timeline drag snaps: to clip edges, other markers, the playhead and the in/out
// points, else to the frame; Ctrl or Cmd held, or snapping off, leaves the frame grid only. Esc,
// a lost pointer or a lost window rolls it back.
import type { MarkerId, RpcClient, RpcError } from "@cutweave/protocol";
import {
  DRAG_THRESHOLD_PX,
  type SnapGuide,
  type SnapOutcome,
  type TimelineMarker,
  type TimelineModel,
  type TimelinePointerEvent,
} from "@cutweave/timeline";
import { type FrameScheduler, GestureSession } from "./gesture.ts";

export interface MarkerDragDeps {
  rpc(): RpcClient | null;
  /** The undo step's name ("Move marker"). */
  label(): string;
  /** The marker whose pin or range bar is at canvas point `(x, y)`. */
  markerAt(x: number, y: number): TimelineMarker | null;
  /** The timeline's projection (its markers are the snapshot the drag snaps against). */
  model(): TimelineModel;
  /** Flicks per CSS pixel at the current zoom. */
  flicksPerPx(): number;
  /** `raw` snapped as a drag snaps it; `model` is the snapshot without the dragged marker. */
  snap(model: TimelineModel, raw: number, suspended: boolean): SnapOutcome;
  /** The snap guide to draw (none: an empty list). */
  guides(guides: readonly SnapGuide[]): void;
  /** A click on a marker goes to it. */
  seek(t: number): void;
  /** The move did not happen: tell the person. */
  refused(error: RpcError): void;
  /** Another gesture is under way: a press starts nothing. */
  busy(): boolean;
  readonly frames?: FrameScheduler;
}

interface Press {
  readonly marker: TimelineMarker;
  readonly x0: number;
  /** The model without the dragged marker (one object for the whole drag: the snap index keeps it). */
  readonly others: TimelineModel;
  session: GestureSession | null;
  /** The last time sent. */
  t: number;
}

const NO_GUIDES: readonly SnapGuide[] = [];

export class MarkerDrag {
  readonly #deps: MarkerDragDeps;
  #press: Press | null = null;
  /** Finishes of earlier drags: the next gesture opens after them. */
  #tail: Promise<unknown> = Promise.resolve();

  constructor(deps: MarkerDragDeps) {
    this.#deps = deps;
  }

  /** Whether a marker is pressed or being dragged. */
  get active(): boolean {
    return this.#press !== null;
  }

  /** Whether a marker is being dragged (the press moved far enough). */
  get dragging(): boolean {
    return this.#press?.session != null;
  }

  /** Resolves when every drag ended so far has reached the engine (tests). */
  settled(): Promise<void> {
    return this.#tail.then(() => undefined);
  }

  /** A pointer event of the canvas. Returns whether it was this drag's (the machine never sees it). */
  handle(e: TimelinePointerEvent): boolean {
    const press = this.#press;
    if (press === null) {
      if (e.type !== "down" || e.button !== 0 || e.hit.zone !== "ruler") return false;
      if (e.shiftKey || e.altKey || this.#deps.busy()) return false;
      const marker = this.#deps.markerAt(e.x, e.y);
      if (marker === null) return false;
      const model = this.#deps.model();
      this.#press = {
        marker,
        x0: e.x,
        others: { ...model, markers: (model.markers ?? []).filter((m) => m.id !== marker.id) },
        session: null,
        t: marker.t,
      };
      return true;
    }
    switch (e.type) {
      case "move":
        this.#move(press, e);
        return true;
      case "up":
        this.#press = null;
        this.#deps.guides(NO_GUIDES);
        if (press.session === null) this.#deps.seek(press.marker.t);
        else this.#finish(press.session, true);
        return true;
      case "cancel":
        this.cancel();
        return true;
      default:
        // `leave` and the rest while the button is down: the canvas holds the pointer.
        return true;
    }
  }

  /** Esc, a lost pointer or a lost window: the marker goes back. */
  cancel(): boolean {
    const press = this.#press;
    if (press === null) return false;
    this.#press = null;
    this.#deps.guides(NO_GUIDES);
    if (press.session !== null) this.#finish(press.session, false);
    return true;
  }

  #move(press: Press, e: TimelinePointerEvent): void {
    if (press.session === null) {
      if (Math.abs(e.x - press.x0) < DRAG_THRESHOLD_PX) return;
      const rpc = this.#deps.rpc();
      if (rpc === null) return;
      press.session = new GestureSession({
        rpc,
        label: this.#deps.label(),
        after: this.#tail,
        ...(this.#deps.frames !== undefined && { frames: this.#deps.frames }),
        onAbort: (error) => {
          if (this.#press === press) {
            this.#press = null;
            this.#deps.guides(NO_GUIDES);
          }
          this.#deps.refused(error);
        },
      });
    }
    const raw = Math.max(0, press.marker.t + (e.x - press.x0) * this.#deps.flicksPerPx());
    const snapped = this.#deps.snap(press.others, raw, e.ctrlKey || e.metaKey);
    const t = Math.max(0, snapped.t);
    this.#deps.guides(snapped.target === null ? NO_GUIDES : [snapped.target]);
    if (t === press.t) return;
    press.t = t;
    press.session.update({
      name: "timeline.move_marker",
      input: { marker: press.marker.id as MarkerId, t },
    });
  }

  #finish(session: GestureSession, commit: boolean): void {
    const done = session.finish(commit).then((outcome) => {
      if (outcome.kind === "refused") this.#deps.refused(outcome.error);
      else if (outcome.kind === "aborted" && !outcome.reported) this.#deps.refused(outcome.error);
    });
    this.#tail = done.catch(() => undefined);
  }
}
