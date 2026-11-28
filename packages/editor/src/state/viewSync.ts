// SPDX-License-Identifier: GPL-3.0-or-later
// Mirrors the persisted part of `viewStore` to the engine (`view.set`, debounced) and restores
// it from `view.get` (WP-1C.04): playhead, zoom, scroll, selection, panel layout (with the
// sequence tabs), the sequence shown (WP-2B.03, sent at once: the engine plays it), and the Source
// view's marks (WP-2A.14, sent at once: a mark is a deliberate act and must survive a quit). The preview
// quality is a preference, not part of it (WP-2H.10). `view.*` commands never touch the document
// and are not undoable.
//
// The view is the project's: when another project opens in the running app, or the project
// closes (`project.opened`, `project.closed`), the store is emptied of the one before at once
// and the view of what is open now is read, exactly as after a handshake (issue #233).

import type { MediaId, RpcClient, SelectionRef, UiState, ViewPatch } from "@cutweave/protocol";
import type { StoreApi } from "zustand/vanilla";
import {
  type EngineViewState,
  type PanelLayout,
  panelLayoutOf,
  type SourceMarksMap,
  type ViewStore,
} from "./viewStore.ts";

/** The engine mirror is flushed this long after the last change (WP-1M.02). */
export const VIEW_SYNC_DEBOUNCE_MS = 500;

/**
 * How often a refused "show this sequence" is sent again on its own (WP-2B.03). The engine's
 * view learns of a new sequence a moment after the command that made it answers, so the choice
 * of a sequence just created can arrive too early; the engine must end up playing what the
 * timeline shows, so this one field does not wait for the next change.
 */
export const ACTIVE_SEQUENCE_RETRIES = 3;

export interface ViewSyncTimers {
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

export interface ViewSyncOptions {
  readonly store: StoreApi<ViewStore>;
  readonly rpc: RpcClient;
  readonly debounceMs?: number;
  readonly timers?: ViewSyncTimers;
  /** A failed `view.get` / `view.set`; the change is retried with the next one. */
  readonly onError?: (error: unknown) => void;
  /**
   * Resolves once the document of what is open now is in the editor's mirror. When a project
   * opens or closes in the running app, its view is read only after that, the order a handshake
   * has (document, then view): the view names clips, and the timeline drops a selection whose
   * clips are not in the document it has.
   */
  readonly documentReady?: () => Promise<unknown>;
}

/** What `viewSync` compares and sends: the engine view plus the layout part of the store. */
interface Mirrored extends EngineViewState {
  readonly panel: PanelLayout;
}

const sameSelection = (a: readonly SelectionRef[], b: readonly SelectionRef[]) =>
  a.length === b.length && a.every((ref, i) => ref.kind === b[i]?.kind && ref.id === b[i]?.id);

function mirrored(state: ViewStore): Mirrored {
  return {
    playhead: state.playhead,
    zoom: state.zoom,
    scroll: state.scroll,
    selection: state.selection,
    sourceMarks: state.sourceMarks,
    activeSequence: state.activeSequence,
    panel: {
      workspaceId: state.workspaceId,
      aspect: state.aspect,
      layouts: state.layouts,
      sequenceTabs: state.sequenceTabs,
    },
  };
}

/** The fields of `now` that differ from `sent`, as a `view.set` patch (`null` when none). */
export function diffView(sent: Mirrored | null, now: Mirrored): ViewPatch | null {
  const patch: ViewPatch = {};
  if (sent === null || sent.playhead !== now.playhead) patch.playhead = now.playhead;
  if (sent === null || sent.zoom !== now.zoom) {
    // `view.set` treats null as "absent": a cleared zoom cannot be sent, so only values go.
    if (now.zoom !== null) patch.zoom = now.zoom;
  }
  if (sent === null || sent.scroll.x !== now.scroll.x || sent.scroll.y !== now.scroll.y) {
    patch.scroll = { x: now.scroll.x, y: now.scroll.y };
  }
  if (sent === null || !sameSelection(sent.selection, now.selection)) {
    patch.selection = [...now.selection];
  }
  if (
    sent === null ||
    sent.panel.workspaceId !== now.panel.workspaceId ||
    sent.panel.aspect !== now.panel.aspect ||
    sent.panel.layouts !== now.panel.layouts ||
    sent.panel.sequenceTabs !== now.panel.sequenceTabs
  ) {
    patch.panel_layout = panelLayoutOf(now.panel);
  }
  // `view.set` treats null as "absent": "none chosen" cannot be sent, and need not be (the
  // engine forgets a deleted sequence itself).
  if ((sent?.activeSequence ?? null) !== now.activeSequence && now.activeSequence !== null) {
    patch.active_sequence = now.activeSequence;
  }
  const marks = diffMarks(sent?.sourceMarks ?? {}, now.sourceMarks);
  if (marks !== null) patch.source_marks = marks;
  return Object.keys(patch).length === 0 ? null : patch;
}

type MarksPatch = NonNullable<ViewPatch["source_marks"]>;

/** The assets whose marks differ, as `view.set`'s per-asset entries (`{}` clears an asset's). */
function diffMarks(sent: SourceMarksMap, now: SourceMarksMap): MarksPatch | null {
  if (sent === now) return null;
  const out: Record<string, { in?: number; out?: number }> = {};
  for (const id of new Set([...Object.keys(sent), ...Object.keys(now)]) as Set<MediaId>) {
    const a = sent[id];
    const b = now[id];
    if ((a?.in ?? null) === (b?.in ?? null) && (a?.out ?? null) === (b?.out ?? null)) continue;
    out[id] = {
      ...(b?.in != null && { in: b.in }),
      ...(b?.out != null && { out: b.out }),
    };
  }
  return Object.keys(out).length === 0 ? null : (out as MarksPatch);
}

export class ViewSync {
  readonly #store: StoreApi<ViewStore>;
  readonly #rpc: RpcClient;
  readonly #debounceMs: number;
  readonly #timers: ViewSyncTimers;
  readonly #onError: (error: unknown) => void;
  readonly #documentReady: (() => Promise<unknown>) | null;
  /** What the engine is known to hold; changes against it are what gets sent. */
  #sent: Mirrored | null = null;
  #timer: unknown = null;
  #flushing: Promise<void> | null = null;
  #dirtyAgain = false;
  /** The engine's own state is being taken in: nothing of it is sent back. */
  #adopting = false;
  #unsubscribe: (() => void) | null = null;
  #offChanged: (() => void) | null = null;
  #offProject: (() => void)[] = [];
  /** Grows with every project change: what was asked of the project before is not taken. */
  #project = 0;
  /** Refused sends of the sequence shown since the last accepted one. */
  #activeRetries = 0;

  constructor(options: ViewSyncOptions) {
    this.#store = options.store;
    this.#rpc = options.rpc;
    this.#debounceMs = options.debounceMs ?? VIEW_SYNC_DEBOUNCE_MS;
    this.#timers = options.timers ?? {
      setTimeout: (callback, ms) => globalThis.setTimeout(callback, ms),
      clearTimeout: (handle) => globalThis.clearTimeout(handle as number),
    };
    this.#onError = options.onError ?? (() => {});
    this.#documentReady = options.documentReady ?? null;
  }

  /**
   * Restores the view from `view.get` (on open and after a reconnect), then starts mirroring
   * changes. Safe to call again: it re-reads the engine's state.
   */
  async restore(): Promise<void> {
    this.#cancelTimer();
    const ui = await this.#rpc.call("view.get", {});
    this.#adopt(ui);
    this.#watch();
  }

  /** Stops mirroring; a change still waiting for its timer is flushed first. */
  async stop(): Promise<void> {
    this.#unsubscribe?.();
    this.#unsubscribe = null;
    this.#offChanged?.();
    this.#offChanged = null;
    for (const off of this.#offProject) off();
    this.#offProject = [];
    if (this.#timer !== null) {
      this.#cancelTimer();
      await this.flush();
    }
  }

  /**
   * Before this app leaves the project (opens another, closes it, makes a new one): what is
   * still waiting for its timer is sent now, so the project keeps it as it does on quit.
   */
  async leave(): Promise<void> {
    if (this.#sent === null) return;
    await this.flush();
  }

  /** Sends what changed since the last flush now. */
  async flush(): Promise<void> {
    this.#cancelTimer();
    if (this.#flushing !== null) {
      this.#dirtyAgain = true;
      return this.#flushing;
    }
    this.#flushing = this.#flushOnce().finally(() => {
      this.#flushing = null;
      if (this.#dirtyAgain) {
        this.#dirtyAgain = false;
        this.#schedule();
      }
    });
    return this.#flushing;
  }

  async #flushOnce(): Promise<void> {
    const now = mirrored(this.#store.getState());
    const patch = diffView(this.#sent, now);
    if (patch === null) {
      return;
    }
    const project = this.#project;
    try {
      await this.#rpc.call("view.set", { patch });
      // Another project opened meanwhile: what the engine holds now is what was read from it.
      if (project === this.#project) this.#sent = now;
      this.#activeRetries = 0;
    } catch (error) {
      this.#onError(error); // `#sent` is unchanged: the next change retries the difference
      // The sequence shown is sent again on its own, a few times (see ACTIVE_SEQUENCE_RETRIES).
      if (
        patch.active_sequence !== undefined &&
        project === this.#project &&
        this.#activeRetries < ACTIVE_SEQUENCE_RETRIES
      ) {
        this.#activeRetries += 1;
        this.#schedule();
      }
    }
  }

  #watch(): void {
    if (this.#unsubscribe === null) {
      this.#unsubscribe = this.#store.subscribe((state, previous) => {
        if (this.#adopting) return; // the engine's own state: nothing to send
        // A mark, and the sequence shown (the engine plays it: WP-2B.03), go at once.
        if (
          state.sourceMarks !== previous.sourceMarks ||
          state.activeSequence !== previous.activeSequence
        ) {
          this.flush().catch(this.#onError);
          return;
        }
        if (
          state.playhead !== previous.playhead ||
          state.zoom !== previous.zoom ||
          state.scroll !== previous.scroll ||
          state.selection !== previous.selection ||
          state.workspaceId !== previous.workspaceId ||
          state.aspect !== previous.aspect ||
          state.layouts !== previous.layouts ||
          state.sequenceTabs !== previous.sequenceTabs
        ) {
          this.#schedule();
        }
      });
    }
    if (this.#offProject.length === 0) {
      this.#offProject = [
        this.#rpc.onUntyped("project.opened", () => void this.#reload()),
        this.#rpc.onUntyped("project.closed", () => void this.#reload()),
      ];
    }
    if (this.#offChanged === null) {
      // Another client (an agent) moved the playhead or changed the selection.
      this.#offChanged = this.#rpc.onUntyped("view.changed", (payload) => {
        if (typeof payload === "object" && payload !== null) {
          this.#adopt(payload as Partial<UiState>);
        }
      });
    }
  }

  /**
   * Another project is open, or none (the engine then holds an untitled document): the view in
   * the store belongs to the one before. It is dropped at once, with what was waiting to be
   * sent, so nothing of it reaches a project that has no such clips or assets (the engine would
   * refuse the whole patch) and nothing of it shows; then the view of what is open now is read,
   * as after a handshake, once its document is in the mirror. The panels stay until the project's own layout is known: one that
   * never saved a layout keeps them, as it does at launch.
   */
  async #reload(): Promise<void> {
    const project = ++this.#project;
    this.#activeRetries = 0;
    this.#cancelTimer();
    this.#dirtyAgain = false;
    this.#adopting = true;
    try {
      this.#store.getState().resetProjectView();
    } finally {
      this.#adopting = false;
    }
    this.#sent = mirrored(this.#store.getState());
    try {
      // The document first, as after a handshake: the engine answers `view.get` and the
      // snapshot in any order, and a selection that arrives before its clips is dropped.
      await this.#documentReady?.().catch(this.#onError);
      if (project !== this.#project) return;
      const ui = await this.#rpc.call("view.get", {});
      if (project !== this.#project) return; // yet another project: its own read is under way
      this.#adopt(ui);
    } catch (error) {
      this.#onError(error);
    }
  }

  /** Takes engine-side state into the store without echoing it back. */
  #adopt(ui: Partial<UiState>): void {
    this.#adopting = true;
    try {
      this.#store.getState().applyEngineView(ui);
    } finally {
      this.#adopting = false;
    }
    this.#sent = mirrored(this.#store.getState());
  }

  #schedule(): void {
    this.#cancelTimer();
    this.#timer = this.#timers.setTimeout(() => {
      this.#timer = null;
      this.flush().catch(this.#onError);
    }, this.#debounceMs);
  }

  #cancelTimer(): void {
    if (this.#timer !== null) {
      this.#timers.clearTimeout(this.#timer);
      this.#timer = null;
    }
  }
}
