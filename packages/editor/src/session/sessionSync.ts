// SPDX-License-Identifier: GPL-3.0-or-later
// Feeds `sessionStore` from the engine: the project status of every handshake (`rpc.hello`),
// the `project.*` events and the user's undo/redo stacks (`edit.history_changed`, read again
// after a handshake and after `doc.reset`, because stacks are not tied to a revision and an
// event can be missed while disconnected).

import type { ActorStacks, ProjectStatus, RpcClient } from "@cutweave/protocol";
import type { StoreApi } from "zustand/vanilla";
import { EMPTY_STACKS, type SessionStore, type UndoStacks } from "./sessionStore.ts";

export interface SessionSyncOptions {
  readonly store: StoreApi<SessionStore>;
  readonly rpc: RpcClient;
  /** The actor of this connection (`rpc.hello`'s `actor`); defaults to `user`. */
  readonly actor?: string;
  /** `project.autosave_failed`: say so (a toast); the message holds paths and OS text only. */
  readonly onAutosaveFailed?: (message: string) => void;
  readonly onError?: (error: unknown) => void;
}

export function stacksOf(stacks: ActorStacks | undefined): UndoStacks {
  if (stacks === undefined) return EMPTY_STACKS;
  return {
    undoDepth: stacks.undo_depth,
    redoDepth: stacks.redo_depth,
    nextUndo: stacks.next_undo?.label ?? null,
    nextRedo: stacks.next_redo?.label ?? null,
  };
}

export class SessionSync {
  readonly #options: SessionSyncOptions;
  #actor: string;
  #offs: (() => void)[] = [];

  constructor(options: SessionSyncOptions) {
    this.#options = options;
    this.#actor = options.actor ?? "user";
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
      rpc.on("project.autosave_failed", ({ message }) => this.#options.onAutosaveFailed?.(message)),
      rpc.on("edit.history_changed", ({ stacks }) => {
        // Another actor's changes do not move this user's stacks.
        if (stacks.actor === this.#actor) state().setStacks(stacksOf(stacks));
      }),
      rpc.on("doc.reset", () => void this.refreshStacks()),
    );
  }

  /** The project status the engine reported in `rpc.hello` (every handshake). */
  applyHello(hello: { readonly actor: string; readonly project: ProjectStatus }): void {
    this.#actor = hello.actor;
    this.#options.store.getState().applyStatus(hello.project);
  }

  /** Reads the undo/redo stacks of this actor. */
  async refreshStacks(): Promise<void> {
    try {
      const page = await this.#options.rpc.call("edit.history", { limit: 0 });
      this.#options.store
        .getState()
        .setStacks(stacksOf(page.stacks.find((s) => s.actor === this.#actor)));
    } catch (error) {
      this.#options.onError?.(error);
    }
  }

  stop(): void {
    for (const off of this.#offs) off();
    this.#offs = [];
  }
}
