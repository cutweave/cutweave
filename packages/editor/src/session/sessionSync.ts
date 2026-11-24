// SPDX-License-Identifier: GPL-3.0-or-later
// Feeds `sessionStore` from the engine: the project status of every handshake (`rpc.hello`),
// the `project.*` events and the user's undo/redo stacks (`edit.history_changed`, read again
// after a handshake and after `doc.reset`, because stacks are not tied to a revision and an
// event can be missed while disconnected).

import type { LatestStacks, ProjectStatus, RpcClient } from "@cutweave/protocol";
import type { StoreApi } from "zustand/vanilla";
import { EMPTY_STACKS, type SessionStore, type UndoStacks } from "./sessionStore.ts";

export interface SessionSyncOptions {
  readonly store: StoreApi<SessionStore>;
  readonly rpc: RpcClient;
  /** `project.autosave_failed`: say so (a toast); the message holds paths and OS text only. */
  readonly onAutosaveFailed?: (message: string) => void;
  readonly onError?: (error: unknown) => void;
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

  constructor(options: SessionSyncOptions) {
    this.#options = options;
  }

  /** Subscribes to the events; call once. Read the state with {@link applyHello} / {@link refresh}. */
  start(): void {
    const { rpc, store } = this.#options;
    const state = () => store.getState();
    this.#offs.push(
      rpc.on("project.opened", (status) => state().applyStatus(status)),
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
      rpc.on("doc.reset", () => void this.refreshStacks()),
    );
  }

  /** The project status the engine reported in `rpc.hello` (every handshake). */
  applyHello(hello: { readonly project: ProjectStatus }): void {
    this.#options.store.getState().applyStatus(hello.project);
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
