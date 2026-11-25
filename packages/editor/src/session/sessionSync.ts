// SPDX-License-Identifier: GPL-3.0-or-later
// Feeds `sessionStore` from the engine: the project status of every handshake (`rpc.hello`),
// the `project.*` events and the user's undo/redo stacks (`edit.history_changed`, read again
// after a handshake and after `doc.reset`, because stacks are not tied to a revision and an
// event can be missed while disconnected).
//
// Opening a project (ADR-0029): `project.opening` starts it, whoever asked; it is done when
// `project.opened` came and the new document is in the mirror (the snapshot `doc.reset` asks
// for), or when `project.open_failed` comes. Until then `opening` is set and the editor shows
// it and takes no edits. An open another actor started, finished or failed, or one the engine
// refused because of unsaved changes, is told to the person (`onOpenNotice`).

import type { LatestStacks, ProjectStatus, RpcClient } from "@cutweave/protocol";
import type { StoreApi } from "zustand/vanilla";
import type { DocStore } from "../state/docStore.ts";
import { EMPTY_STACKS, type SessionStore, type UndoStacks } from "./sessionStore.ts";

/** The actor of this app's own connection (the UI credential). */
export const APP_ACTOR = "user";

/** What the person is told about an open an agent or a plugin asked for. */
export type OpenNotice =
  | {
      readonly kind: "opened";
      readonly actor: string;
      readonly path: string;
      /** The opened project's name. */
      readonly name: string;
    }
  | {
      readonly kind: "refused" | "failed";
      readonly actor: string;
      readonly path: string;
      /** The project that stayed open (its unsaved changes refused a `refused` open). */
      readonly current: string;
      /** What the engine said. */
      readonly message: string;
    };

export interface SessionSyncOptions {
  readonly store: StoreApi<SessionStore>;
  readonly rpc: RpcClient;
  /**
   * The document mirror. An open is done once the new document is in it; without it, at
   * `project.opened`.
   */
  readonly doc?: StoreApi<DocStore>;
  /** `project.autosave_failed`: say so (a toast); the message holds paths and OS text only. */
  readonly onAutosaveFailed?: (message: string) => void;
  /** An agent's or a plugin's open opened, was refused or failed: say so (an agent toast). */
  readonly onOpenNotice?: (notice: OpenNotice) => void;
  readonly onError?: (error: unknown) => void;
}

/** The open under way, from `project.opening` until it is done. */
interface PendingOpen {
  readonly path: string;
  readonly actor: string;
  /** `doc.reset` came: the next mirror change is the new document. */
  reset: boolean;
  /** The new document is in the mirror. */
  docIn: boolean;
  /** `project.opened` came, with the project's name. */
  opened: string | null;
}

/** The shared view of the history (ADR-0019): what `*` would undo and redo next, and whose it is. */
export function stacksOf(latest: LatestStacks | undefined): UndoStacks {
  if (latest === undefined) return EMPTY_STACKS;
  return {
    undoDepth: latest.undo_depth,
    redoDepth: latest.redo_depth,
    nextUndo: latest.next_undo?.label ?? null,
    nextRedo: latest.next_redo?.label ?? null,
    nextUndoActor: latest.next_undo?.actor ?? null,
    nextRedoActor: latest.next_redo?.actor ?? null,
  };
}

export class SessionSync {
  readonly #options: SessionSyncOptions;
  #offs: (() => void)[] = [];
  #open: PendingOpen | null = null;

  constructor(options: SessionSyncOptions) {
    this.#options = options;
  }

  /** Subscribes to the events; call once. Read the state with {@link applyHello} / {@link refresh}. */
  start(): void {
    const { rpc, store } = this.#options;
    const state = () => store.getState();
    this.#offs.push(
      rpc.on("project.opening", ({ path, actor }) => {
        const docIn = this.#options.doc === undefined;
        this.#open = { path, actor, reset: false, docIn, opened: null };
        state().setOpening({ path, actor });
      }),
      rpc.on("project.opened", (status) => {
        state().applyStatus(status);
        if (this.#open !== null) {
          this.#open.opened = status.name;
          this.#settleOpen();
        }
      }),
      rpc.on("project.open_failed", ({ path, actor, reason, message }) => {
        this.#open = null;
        if (state().opening !== null) state().setOpening(null);
        if (actor === APP_ACTOR) return; // the app's own open says it itself (ProjectController)
        const kind = reason === "unsaved_changes" ? "refused" : "failed";
        this.#options.onOpenNotice?.({ kind, actor, path, current: this.#currentName(), message });
      }),
      rpc.on("project.closed", () => state().applyClosed()),
      rpc.on("project.dirty", ({ dirty }) => state().setDirty(dirty)),
      rpc.on("project.recovery_available", (payload) => state().setRecovery(payload)),
      rpc.on("project.autosave_failed", ({ message }) => {
        state().setAutosaveFailed(true);
        this.#options.onAutosaveFailed?.(message);
      }),
      rpc.on("project.saved", () => state().setAutosaveFailed(false)),
      // Whichever actor's stack moved, the shared view (`latest`) may have changed.
      rpc.on("edit.history_changed", ({ latest }) => state().setStacks(stacksOf(latest))),
      rpc.on("doc.reset", () => {
        if (this.#open !== null) this.#open.reset = true;
        void this.refreshStacks();
      }),
    );
    const doc = this.#options.doc;
    if (doc !== undefined) {
      this.#offs.push(
        doc.subscribe((next, prev) => {
          const open = this.#open;
          if (open === null || !open.reset || next.project === prev.project) return;
          open.docIn = true;
          this.#settleOpen();
        }),
      );
    }
  }

  /** The project status the engine reported in `rpc.hello` (every handshake). */
  applyHello(hello: { readonly project: ProjectStatus }): void {
    // A new handshake (an engine restart): an open that was under way is over either way.
    this.#open = null;
    const store = this.#options.store.getState();
    if (store.opening !== null) store.setOpening(null);
    store.applyStatus(hello.project);
  }

  /** Ends the open once `project.opened` came and its document is in the mirror. */
  #settleOpen(): void {
    const open = this.#open;
    if (open === null || open.opened === null || !open.docIn) return;
    this.#open = null;
    this.#options.store.getState().setOpening(null);
    if (open.actor === APP_ACTOR) return;
    this.#options.onOpenNotice?.({
      kind: "opened",
      actor: open.actor,
      path: open.path,
      name: open.opened,
    });
  }

  /** The open project's name: the document's own, then the engine's ("" when it has none). */
  #currentName(): string {
    const named = this.#options.doc?.getState().project?.meta.name;
    return named !== undefined && named !== "" ? named : this.#options.store.getState().name;
  }

  /** Reads what Undo and Redo would do next. */
  async refreshStacks(): Promise<void> {
    try {
      const page = await this.#options.rpc.call("edit.history", { limit: 0 });
      this.#options.store.getState().setStacks(stacksOf(page.latest));
    } catch (error) {
      this.#options.onError?.(error);
    }
  }

  stop(): void {
    for (const off of this.#offs) off();
    this.#offs = [];
  }
}
