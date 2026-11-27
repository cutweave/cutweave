// SPDX-License-Identifier: GPL-3.0-or-later
// Mirrors the persisted part of `viewStore` to the engine (`view.set`, debounced) and restores
// it from `view.get` (WP-1C.04): playhead, zoom, scroll, selection, panel layout, and the Source
// view's marks (WP-2A.14, sent at once: a mark is a deliberate act and must survive a quit). The preview
// quality is a preference, not part of it (WP-2H.10). `view.*` commands never touch the document
// and are not undoable.

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
    panel: { workspaceId: state.workspaceId, aspect: state.aspect, layouts: state.layouts },
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
    sent.panel.layouts !== now.panel.layouts
  ) {
    patch.panel_layout = panelLayoutOf(now.panel);
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
  /** What the engine is known to hold; changes against it are what gets sent. */
  #sent: Mirrored | null = null;
  #timer: unknown = null;
  #flushing: Promise<void> | null = null;
  #dirtyAgain = false;
  /** The engine's own state is being taken in: nothing of it is sent back. */
  #adopting = false;
  #unsubscribe: (() => void) | null = null;
  #offChanged: (() => void) | null = null;

  constructor(options: ViewSyncOptions) {
    this.#store = options.store;
    this.#rpc = options.rpc;
    this.#debounceMs = options.debounceMs ?? VIEW_SYNC_DEBOUNCE_MS;
    this.#timers = options.timers ?? {
      setTimeout: (callback, ms) => globalThis.setTimeout(callback, ms),
      clearTimeout: (handle) => globalThis.clearTimeout(handle as number),
    };
    this.#onError = options.onError ?? (() => {});
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
    if (this.#timer !== null) {
      this.#cancelTimer();
      await this.flush();
    }
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
    try {
      await this.#rpc.call("view.set", { patch });
      this.#sent = now;
    } catch (error) {
      this.#onError(error); // `#sent` is unchanged: the next change retries the difference
    }
  }

  #watch(): void {
    if (this.#unsubscribe === null) {
      this.#unsubscribe = this.#store.subscribe((state, previous) => {
        if (state.sourceMarks !== previous.sourceMarks && !this.#adopting) {
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
          state.layouts !== previous.layouts
        ) {
          this.#schedule();
        }
      });
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
