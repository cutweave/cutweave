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
  type HitResult,
  type Intent,
  type InteractionEnv,
  type InteractionEvent,
  type InteractionState,
  initialState,
  type KeyInput,
  type MediaDrag,
  type Rect,
  reduce,
  type TimelinePointerEvent,
  type ViewState as TimelineViewState,
} from "@cutweave/timeline";
import type { StoreApi } from "zustand";
import { createStore } from "zustand/vanilla";
import type { EngineStoreState } from "../engine/engineStore.ts";
import { insertMedia } from "../media/insert.ts";
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
  /** What is under canvas point `(x, y)`: the drop target of a dragged media card. */
  hitTest(x: number, y: number): HitResult;
  /** Canvas rect of a track's name in its header (where the rename input goes). */
  trackNameRect?(trackId: string): Rect | null;
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
  /**
   * Moves the playhead (ruler scrubbing, clicks on the ruler): the transport's latest-wins
   * `view.seek` (WP-1M.04). Without it the view's playhead moves alone.
   */
  readonly seek?: (t: number) => void;
}

interface OpenGesture {
  readonly session: GestureSession;
  readonly edit: TimelineEdit;
}

const NO_GUIDES: readonly number[] = [];

/** The track whose name is being edited in place (`trackId`), and why the last commit was refused. */
export interface RenameState {
  readonly trackId: string | null;
  /** The reason the typed name was refused (shown next to the input), or `null`. */
  readonly reason: string | null;
}

/** How a rename ended: `renamed` sent `track.set`; the others sent nothing. */
export type RenameOutcome = "renamed" | "unchanged" | "refused" | "failed";

const trackRef = (id: string): SelectionRef => ({ kind: "track", id: id as TrackId });

export class TimelineController {
  /** Snap guides and the marquee, for the canvas. */
  readonly overlay: StoreApi<TimelineOverlay> = createStore<TimelineOverlay>(() => ({
    snapGuides: NO_GUIDES,
    marquee: null,
  }));

  /** The in-place track rename (WP-2H.x, issue 149): the panel draws the input while it is open. */
  readonly rename: StoreApi<RenameState> = createStore<RenameState>(() => ({
    trackId: null,
    reason: null,
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
    const trackId = e.hit.trackId;
    if (trackId !== null && e.hit.zone === "header-name") {
      if (e.type === "dblclick") {
        this.beginRename(trackId);
        return;
      }
    }
    // A press on a track header selects the track (F2 then renames it).
    if (
      e.type === "down" &&
      e.button === 0 &&
      trackId !== null &&
      e.hit.zone.startsWith("header")
    ) {
      if (e.hit.zone !== "header-toggle")
        this.#deps.view.getState().setSelection([trackRef(trackId)]);
    }
    const event = fromPointerEvent(e);
    if (event !== null) this.send(event);
  }

  /** A key press in the timeline panel. Returns whether it was handled (prevent the default). */
  handleKey(e: KeyInput): boolean {
    // Field by field: a DOM `KeyboardEvent` keeps these as prototype getters, which `...` skips.
    return this.send({
      type: "key",
      key: e.key,
      shiftKey: e.shiftKey === true,
      altKey: e.altKey === true,
      ctrlKey: e.ctrlKey === true,
      metaKey: e.metaKey === true,
    });
  }

  split(): void {
    this.send({ type: "split" });
  }

  // ---- track rename (issue 149) --------------------------------------------------------------
  // `beginRename(trackId)` is the entry point for every way to start a rename: the header's
  // double-click and F2 call it, and the track header's context menu (WP-2H.12/2H.13) will too.

  /** Opens the in-place name input for `trackId`. A locked track can be renamed: lock guards clips. */
  beginRename(trackId: string): void {
    const model = selectTimelineModel(this.#deps.doc.getState().project);
    if (!model.tracks.some((t) => t.id === trackId)) return;
    this.rename.setState({ trackId, reason: null });
  }

  /** F2: renames the selected track, or the track of the selected clip. */
  renameSelectedTrack(): void {
    const { selection } = this.#deps.view.getState();
    const model = selectTimelineModel(this.#deps.doc.getState().project);
    const picked = selection.find((r) => r.kind === "track");
    if (picked !== undefined) {
      this.beginRename(picked.id);
      return;
    }
    const item = selection.find((r) => r.kind === "item");
    const owner = item === undefined ? undefined : model.items.find((i) => i.id === item.id);
    if (owner !== undefined) this.beginRename(owner.trackId);
  }

  /** Esc: closes the input, the old name stays. */
  cancelRename(): void {
    this.rename.setState({ trackId: null, reason: null });
  }

  /** The name currently shown for the track being renamed (the input's starting text). */
  renameTarget(): { readonly trackId: string; readonly name: string } | null {
    const { trackId } = this.rename.getState();
    if (trackId === null) return null;
    const model = selectTimelineModel(this.#deps.doc.getState().project);
    const track = model.tracks.find((t) => t.id === trackId);
    return track === undefined ? null : { trackId, name: track.name };
  }

  /**
   * Enter or blur: `track.set{track, name}` with the trimmed name (one undo step). An empty name is
   * refused and nothing is sent: on Enter the input stays open with the reason (`inline`), on blur it
   * closes and the reason is a toast. The same name sends nothing.
   */
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
      this.#notify("rename", error);
      return "failed";
    }
  }

  /** The canvas rect of a track's name (the input goes over it), or `null` while it is off screen. */
  trackNameRect(trackId: string): Rect | null {
    return this.#geometry?.trackNameRect?.(trackId) ?? null;
  }

  deleteSelection(): void {
    this.send({ type: "delete" });
  }

  toggleSnapping(): void {
    this.send({ type: "toggle-snapping" });
  }

  /**
   * A media card is dragged over the canvas at canvas point `(x, y)` (the panel converts client
   * coordinates). Draws the snapped drop guide. Returns whether a drop would land (a track of
   * the right kind exists), which decides the drag's `dropEffect`.
   */
  mediaDragOver(x: number, y: number, media: MediaDrag): boolean {
    const hit = this.#geometry?.hitTest(x, y);
    if (hit === undefined) return false;
    this.send({ type: "media-drag-over", x, y, hit, media });
    return this.#state.mode.name === "dropping-media";
  }

  /** The card was released at canvas point `(x, y)`: inserts it at the snapped time and track. */
  mediaDrop(x: number, y: number, media: MediaDrag): void {
    const hit = this.#geometry?.hitTest(x, y);
    if (hit === undefined) return;
    this.send({ type: "media-drop", x, y, hit, media });
  }

  /** The card left the canvas or the drag ended elsewhere. */
  mediaDragLeave(): void {
    this.send({ type: "media-drag-leave" });
  }

  /** The pointer went away (window blur, unmount): cancel any gesture. */
  cancel(): void {
    this.send({ type: "pointer-cancel" });
  }

  /** The panel unmounts: roll back any open gesture (the view detaches through `attach(null)`). */
  dispose(): void {
    this.cancel();
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
      case "insert":
        this.#enqueue(async () => {
          await insertMedia(this.#deps, intent.media, intent.trackId, intent.at);
        });
        return;
      case "split":
        this.#enqueue(() => this.#split(intent.items, intent.at));
        return;
      case "delete":
        this.#enqueue(() => this.#delete(intent.items));
        return;
      case "seek":
        if (this.#deps.seek) this.#deps.seek(intent.at);
        else view.setPlayhead(intent.at);
        return;
      case "scroll":
        view.setScroll(intent.scrollX, intent.scrollY);
        return;
      case "set-snapping":
        view.setSnapping(intent.on);
        return;
      case "rename-track":
        this.renameSelectedTrack();
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
