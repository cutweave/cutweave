// SPDX-License-Identifier: GPL-3.0-or-later
// The editor side of timeline interactions (WP-1M.06): feeds pointer and key input through the
// `@cutweave/timeline` state machine and carries out its intents. Edits become engine commands
// (arch §5.1: the UI never touches `docStore`; the change comes back as `doc.patched`); move and
// trim gestures run as gesture transactions (`gesture.ts`); playhead, scroll, selection and
// snapping go to `viewStore`. Engine refusals revert and show a toast. Framework-free, so the
// flows run against a real engine in Vitest; `useTimelineIntents` binds it to React.
import type { ItemId, RpcClient, SelectionRef, TrackId } from "@cutweave/protocol";
import {
  DEFAULT_VIEW_STATE,
  fromPointerEvent,
  type Intent,
  type InteractionEnv,
  type InteractionEvent,
  type InteractionState,
  initialState,
  type KeyInput,
  type Rect,
  reduce,
  type TimelinePointerEvent,
  type ViewState as TimelineViewState,
} from "@cutweave/timeline";
import type { StoreApi } from "zustand";
import { createStore } from "zustand/vanilla";
import type { EngineStoreState } from "../engine/engineStore.ts";
import type { DocStore } from "../state/docStore.ts";
import { selectTimelineModel } from "../state/selectors.ts";
import type { ToastStore } from "../state/toastStore.ts";
import type { ViewStore } from "../state/viewStore.ts";
import { describeRefusal, FAILED_TITLE, type TimelineEdit, type Translate } from "./errors.ts";
import { type FrameScheduler, GestureSession, toRpcError } from "./gesture.ts";

/** What the controller needs from the live `TimelineView`. */
export interface TimelineGeometry {
  /** The view's zoom and scroll as it applied them (clamped). */
  getViewState(): Pick<TimelineViewState, "pxPerSec" | "scrollX" | "scrollY">;
  /** Items inside a canvas rectangle (marquee). */
  itemsInRect(rect: Rect): string[];
}

/** Transient drawing the timeline shows while a gesture runs. */
export interface TimelineOverlay {
  readonly snapGuides: readonly number[];
  readonly marquee: Rect | null;
}

export interface TimelineControllerDeps {
  readonly doc: StoreApi<DocStore>;
  readonly view: StoreApi<ViewStore>;
  readonly engine: StoreApi<EngineStoreState>;
  readonly toasts: StoreApi<ToastStore>;
  readonly t: Translate;
  readonly frames?: FrameScheduler;
}

interface OpenGesture {
  readonly session: GestureSession;
  readonly edit: TimelineEdit;
}

const NO_GUIDES: readonly number[] = [];

export class TimelineController {
  /** Snap guides and the marquee, for the canvas. */
  readonly overlay: StoreApi<TimelineOverlay> = createStore<TimelineOverlay>(() => ({
    snapGuides: NO_GUIDES,
    marquee: null,
  }));

  readonly #deps: TimelineControllerDeps;
  #state: InteractionState = initialState();
  #geometry: TimelineGeometry | null = null;
  #gesture: OpenGesture | null = null;
  #lastSession: GestureSession | null = null;
  /** Engine work in order: a gesture opens only after the previous one (and splits, deletes) ended. */
  #queue: Promise<unknown> = Promise.resolve();
  #selectionRefs: readonly SelectionRef[] | null = null;
  #selectionIds: readonly string[] = [];

  constructor(deps: TimelineControllerDeps) {
    this.#deps = deps;
  }

  /** The machine's current state (tests, debugging). */
  get state(): InteractionState {
    return this.#state;
  }

  /** The most recent gesture's session (its `updates` count is the engine traffic). */
  get lastGesture(): GestureSession | null {
    return this.#lastSession;
  }

  /** Connects the live view (null when it unmounts). */
  attach(geometry: TimelineGeometry | null): void {
    this.#geometry = geometry;
  }

  /** Resolves when every edit started so far has finished (tests). */
  settled(): Promise<void> {
    return this.#queue.then(() => undefined);
  }

  /** A pointer event from `TimelineView.onPointer`. */
  handlePointer(e: TimelinePointerEvent): void {
    const event = fromPointerEvent(e);
    if (event !== null) this.send(event);
  }

  /** A key press in the timeline panel. Returns whether it was handled (prevent the default). */
  handleKey(e: KeyInput): boolean {
    return this.send({ type: "key", ...e });
  }

  split(): void {
    this.send({ type: "split" });
  }

  deleteSelection(): void {
    this.send({ type: "delete" });
  }

  toggleSnapping(): void {
    this.send({ type: "toggle-snapping" });
  }

  /** The pointer went away (window blur, unmount): cancel any gesture. */
  cancel(): void {
    this.send({ type: "pointer-cancel" });
  }

  dispose(): void {
    this.cancel();
    this.#geometry = null;
  }

  /** Runs one event through the machine with fresh env and carries out its intents. */
  send(event: InteractionEvent): boolean {
    const before = reduce(this.#state, { type: "env", env: this.#env() }).state;
    const { state, intents } = reduce(before, event);
    this.#state = state;
    for (const intent of intents) this.#apply(intent);
    return state !== before || intents.length > 0;
  }

  // ---- environment --------------------------------------------------------------------------

  #env(): InteractionEnv {
    const view = this.#deps.view.getState();
    const live = this.#geometry?.getViewState();
    return {
      model: selectTimelineModel(this.#deps.doc.getState().project),
      pxPerSec: live?.pxPerSec ?? view.zoom ?? DEFAULT_VIEW_STATE.pxPerSec,
      scrollX: live?.scrollX ?? view.scroll.x,
      scrollY: live?.scrollY ?? view.scroll.y,
      playhead: view.playhead,
      selection: this.#itemSelection(view.selection),
      snapping: view.snapping,
    };
  }

  #itemSelection(refs: readonly SelectionRef[]): readonly string[] {
    if (refs !== this.#selectionRefs) {
      this.#selectionRefs = refs;
      this.#selectionIds = refs.filter((r) => r.kind === "item").map((r) => r.id);
    }
    return this.#selectionIds;
  }

  #select(ids: readonly string[]): void {
    this.#deps.view
      .getState()
      .setSelection(ids.map((id) => ({ kind: "item", id: id as ItemId }) as SelectionRef));
  }

  // ---- intents ------------------------------------------------------------------------------

  #apply(intent: Intent): void {
    const view = this.#deps.view.getState();
    switch (intent.kind) {
      case "select":
        this.#select(intent.ids);
        return;
      case "select-rect": {
        const inside = this.#geometry?.itemsInRect(intent.rect) ?? [];
        const current = intent.additive ? this.#itemSelection(view.selection) : [];
        this.#select([...current, ...inside.filter((id) => !current.includes(id))]);
        return;
      }
      case "marquee":
        this.overlay.setState({ marquee: intent.rect });
        return;
      case "snap-guides":
        this.overlay.setState({ snapGuides: intent.times.length === 0 ? NO_GUIDES : intent.times });
        return;
      case "begin-gesture":
        this.#begin(intent.gesture);
        return;
      case "move":
        this.#gesture?.session.update({
          name: "item.move",
          input: {
            item: intent.item as ItemId,
            start: intent.at,
            to_track: intent.toTrack as TrackId,
            ...(intent.unlink && { unlink: true }),
          },
        });
        return;
      case "trim":
        this.#gesture?.session.update({
          name: "clip.trim",
          input: {
            item: intent.item as ItemId,
            edge: intent.edge,
            to: intent.at,
            ...(intent.unlink && { unlink: true }),
          },
        });
        return;
      case "commit-gesture":
        this.#finish(true);
        return;
      case "cancel-gesture":
        this.#finish(false);
        return;
      case "split":
        this.#enqueue(() => this.#split(intent.items, intent.at));
        return;
      case "delete":
        this.#enqueue(() => this.#delete(intent.items));
        return;
      case "seek":
        view.setPlayhead(intent.at);
        return;
      case "scroll":
        view.setScroll(intent.scrollX, intent.scrollY);
        return;
      case "set-snapping":
        view.setSnapping(intent.on);
        return;
    }
  }

  #rpc(): RpcClient | null {
    return this.#deps.engine.getState().rpc;
  }

  #begin(gesture: "move" | "trim"): void {
    const rpc = this.#rpc();
    if (rpc === null) {
      this.send({ type: "gesture-aborted" });
      return;
    }
    const edit: TimelineEdit = gesture;
    const session: GestureSession = new GestureSession({
      rpc,
      label: this.#deps.t(gesture === "move" ? "timeline.history.move" : "timeline.history.trim"),
      after: this.#queue,
      ...(this.#deps.frames && { frames: this.#deps.frames }),
      onAbort: (error) => {
        if (this.#gesture?.session === session) {
          this.#gesture = null;
          this.send({ type: "gesture-aborted" });
        }
        this.#notify(edit, error);
      },
    });
    this.#gesture = { session, edit };
    this.#lastSession = session;
  }

  #finish(commit: boolean): void {
    const open = this.#gesture;
    this.#gesture = null;
    if (open === null) return;
    const done = open.session.finish(commit).then((outcome) => {
      if (outcome.kind === "refused" || (outcome.kind === "aborted" && !outcome.reported)) {
        this.#notify(open.edit, outcome.error);
      }
    });
    this.#queue = this.#queue.then(() => done);
  }

  #enqueue(work: () => Promise<void>): void {
    this.#queue = this.#queue.then(work, work);
  }

  async #split(items: readonly string[], at: number): Promise<void> {
    const rpc = this.#rpc();
    if (rpc === null) return;
    try {
      if (items.length === 1) {
        await rpc.call("clip.split", { item: items[0] as ItemId, at });
        return;
      }
      // Several clips split in one batch: one undo step.
      const { tx } = await rpc.call("edit.begin_tx", {
        kind: "batch",
        label: this.#deps.t("timeline.history.split"),
      });
      try {
        for (const item of items)
          await rpc.call("clip.split", { item: item as ItemId, at }, { tx });
        await rpc.call("edit.commit_tx", { tx });
      } catch (error) {
        await rpc.call("edit.cancel_tx", { tx }).catch(() => undefined);
        throw error;
      }
    } catch (error) {
      this.#notify("split", error);
    }
  }

  async #delete(items: readonly string[]): Promise<void> {
    const rpc = this.#rpc();
    if (rpc === null) return;
    try {
      const out = await rpc.call("item.delete", { items: items as ItemId[] });
      const gone = new Set<string>(out.deleted);
      const view = this.#deps.view.getState();
      view.setSelection(view.selection.filter((r) => r.kind !== "item" || !gone.has(r.id)));
    } catch (error) {
      this.#notify("delete", error);
    }
  }

  #notify(edit: TimelineEdit, error: unknown): void {
    const { t, toasts } = this.#deps;
    toasts.getState().push({
      tone: "warning",
      title: t(FAILED_TITLE[edit]),
      message: describeRefusal(toRpcError(error), t),
    });
  }
}
