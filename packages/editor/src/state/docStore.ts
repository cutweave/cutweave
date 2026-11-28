// SPDX-License-Identifier: GPL-3.0-or-later
import {
  type ApplyResult,
  applyDocEvent,
  type DocPatched,
  type DocSink,
  type DocState,
  docFromSnapshot,
  EMPTY_DOC,
  type ProjectSnapshot,
} from "@cutweave/protocol";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

/**
 * The editor's read-only projection of the engine document (arch §16): `{rev, project}`.
 *
 * The UI never writes it. The only ways in are `applyEvent` (a `doc.patched` event, applied
 * under the revision-gap rules of `@cutweave/protocol`) and `resync` (a `project.snapshot`);
 * both are called by `DocSync`. Every edit goes through a command (`useCommand`), and the
 * result comes back as an event. `republish` changes nothing of the document: it only tells
 * the subscribers to read it again.
 */
export interface DocStore extends DocState {
  /**
   * Publishes the same document again under a new object (same revision, same contents, every
   * part shared), so what is derived from "the project" is derived again. For a change of what
   * the editor shows of it (another sequence, WP-2B.03), never for a change of the document.
   */
  republish(): void;
  /** Applies one `doc.patched` event; `gap` tells the caller to resync. */
  applyEvent(event: DocPatched): ApplyResult;
  /** Replaces the projection with a snapshot. */
  resync(snapshot: ProjectSnapshot): void;
}

export function createDocStore() {
  return createStore<DocStore>()((set, get) => ({
    ...EMPTY_DOC,
    applyEvent(event) {
      const { status, rev, committedRev, project } = get();
      const outcome = applyDocEvent({ status, rev, committedRev, project }, event);
      if (outcome.result === "applied") {
        set(outcome.state);
      }
      return outcome.result;
    },
    resync(snapshot) {
      set(docFromSnapshot(snapshot));
    },
    republish() {
      const { project } = get();
      if (project !== null) set({ project: { ...project } });
    },
  }));
}

export const docStore = createDocStore();

/** The store as a `DocSink` for `DocSync`. */
export function docSink(store: ReturnType<typeof createDocStore> = docStore): DocSink {
  return {
    applyEvent: (event) => store.getState().applyEvent(event),
    resync: (snapshot) => store.getState().resync(snapshot),
  };
}

export function useDocStore<T>(selector: (state: DocStore) => T): T {
  return useStore(docStore, selector);
}
