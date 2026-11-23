// SPDX-License-Identifier: GPL-3.0-or-later
// The UI's copy of the engine document: `{rev, project}` plus the rules for keeping it current
// from `doc.patched` events (rpc.md, "Resync: the revision-gap protocol"). Pure functions, so
// the editor's zustand `docStore` and the tests share one implementation.

import type { DocPatched, Project } from "./generated/types.ts";
import { applyPatch, type Json } from "./patch.ts";
import type { ProjectSnapshot } from "./rpc.ts";

export interface DocState {
  /** `empty` until the first snapshot arrives. */
  readonly status: "empty" | "ready";
  /** The live revision the mirror is at. */
  readonly rev: number;
  readonly committedRev: number;
  readonly project: Project | null;
}

export const EMPTY_DOC: DocState = { status: "empty", rev: 0, committedRev: 0, project: null };

/**
 * What an event did to the mirror:
 * - `applied`: the ops were applied and `rev` advanced;
 * - `stale`: the event is not newer than the mirror (it is already in the snapshot): dropped;
 * - `gap`: revisions were missed (or the ops do not apply): fetch `project.snapshot` again.
 */
export type ApplyResult = "applied" | "stale" | "gap";

export interface ApplyOutcome {
  readonly state: DocState;
  readonly result: ApplyResult;
}

/** Mirror state for a `project.snapshot` answer. */
export function docFromSnapshot(snapshot: ProjectSnapshot): DocState {
  return {
    status: "ready",
    rev: snapshot.rev,
    committedRev: snapshot.committed_rev,
    project: snapshot.project,
  };
}

/** Applies one `doc.patched` event under the revision-gap rules; never throws. */
export function applyDocEvent(state: DocState, event: DocPatched): ApplyOutcome {
  if (state.status !== "ready" || state.project === null) {
    return { state, result: "gap" };
  }
  if (event.rev <= state.rev) {
    return { state, result: "stale" };
  }
  if (event.prev_rev !== state.rev) {
    return { state, result: "gap" };
  }
  try {
    const project = applyPatch(state.project as unknown as Json, event.ops) as unknown as Project;
    return {
      state: { status: "ready", rev: event.rev, committedRev: event.committed_rev, project },
      result: "applied",
    };
  } catch {
    return { state, result: "gap" };
  }
}

/** Where a {@link DocSync} writes: the editor's `docStore`, or a {@link DocMirror} in tests. */
export interface DocSink {
  /** Applies an event; the result tells the driver whether to resync. */
  applyEvent(event: DocPatched): ApplyResult;
  /** Replaces the whole mirror with a snapshot. */
  resync(snapshot: ProjectSnapshot): void;
}

/** A framework-free mirror: state plus change listeners. */
export class DocMirror implements DocSink {
  #state: DocState = EMPTY_DOC;
  readonly #listeners = new Set<(state: DocState) => void>();

  get state(): DocState {
    return this.#state;
  }

  applyEvent(event: DocPatched): ApplyResult {
    const outcome = applyDocEvent(this.#state, event);
    this.#set(outcome.state);
    return outcome.result;
  }

  resync(snapshot: ProjectSnapshot): void {
    this.#set(docFromSnapshot(snapshot));
  }

  subscribe(listener: (state: DocState) => void): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }

  #set(state: DocState): void {
    if (state === this.#state) {
      return;
    }
    this.#state = state;
    for (const listener of [...this.#listeners]) {
      listener(state);
    }
  }
}
