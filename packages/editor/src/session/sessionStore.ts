// SPDX-License-Identifier: GPL-3.0-or-later
// What the editor knows about the engine's project session (WP-1M.07): the file it is bound to,
// whether it has unsaved changes, a pending crash-recovery decision, read-only state, and the
// undo/redo stacks of the user. Read-only projection of engine events (`SessionSync` is the
// only writer); every change goes through a command.
//
// "Dirty" is the engine's `project.dirty` event, never `rev != committed_rev`: after an
// `edit.cancel_tx` the live revision stays ahead of the committed one although the content is
// the committed document (WP-1M.06 handoff).

import type { Project, ProjectStatus, RecoveryInfo } from "@cutweave/protocol";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

export interface UndoStacks {
  readonly undoDepth: number;
  readonly redoDepth: number;
  /** The label "Undo <label>" would revert, as the engine logged it (a command name or a tx label). */
  readonly nextUndo: string | null;
  readonly nextRedo: string | null;
  /** Who made the change `nextUndo` names (an actor id such as `user` or `agent:claude`). */
  readonly nextUndoActor: string | null;
  /** Who undid the change `nextRedo` names. */
  readonly nextRedoActor: string | null;
}

export const EMPTY_STACKS: UndoStacks = {
  undoDepth: 0,
  redoDepth: 0,
  nextUndo: null,
  nextRedo: null,
  nextUndoActor: null,
  nextRedoActor: null,
};

export interface SessionState {
  /** False until the first status (`rpc.hello`) arrived: the title and menus say nothing yet. */
  readonly known: boolean;
  /** The project document path, or `null` for an untitled document. */
  readonly path: string | null;
  /** The engine's project name (the document's own name wins when it is loaded). */
  readonly name: string;
  readonly dirty: boolean;
  /** Why edits are refused (a project from a newer version), or `null`. */
  readonly readOnly: string | null;
  /** Unsaved work from a crashed session, waiting for Restore / Discard. */
  readonly recovery: RecoveryInfo | null;
  readonly stacks: UndoStacks;
  /** The engine's last autosave failed and nothing was saved since (`project.autosave_failed`). */
  readonly autosaveFailed: boolean;
}

export interface SessionStore extends SessionState {
  applyStatus(status: ProjectStatus): void;
  /** `project.closed`: the engine continues with an untitled document. */
  applyClosed(): void;
  /** `project.save_as` answered: the session is bound to `path` and has nothing unsaved. */
  applySaved(path: string): void;
  setDirty(dirty: boolean): void;
  setRecovery(recovery: RecoveryInfo | null): void;
  setStacks(stacks: UndoStacks): void;
  setAutosaveFailed(failed: boolean): void;
}

export const initialSessionState: SessionState = {
  known: false,
  path: null,
  name: "",
  dirty: false,
  readOnly: null,
  recovery: null,
  stacks: EMPTY_STACKS,
  autosaveFailed: false,
};

export function createSessionStore() {
  return createStore<SessionStore>()((set) => ({
    ...initialSessionState,
    applyStatus: (s) =>
      set({
        known: true,
        path: s.path,
        name: s.name,
        dirty: s.dirty,
        readOnly: s.read_only,
        recovery: s.recovery,
        autosaveFailed: false,
      }),
    applyClosed: () =>
      set({
        known: true,
        path: null,
        dirty: false,
        readOnly: null,
        recovery: null,
        autosaveFailed: false,
      }),
    applySaved: (path) => set({ known: true, path, dirty: false, autosaveFailed: false }),
    setDirty: (dirty) => set({ dirty }),
    setRecovery: (recovery) => set({ recovery }),
    setStacks: (stacks) => set({ stacks }),
    setAutosaveFailed: (autosaveFailed) => set({ autosaveFailed }),
  }));
}

export const sessionStore = createSessionStore();

export function useSessionStore<T>(selector: (state: SessionStore) => T): T {
  return useStore(sessionStore, selector);
}

/** Whether the document holds media or clips (an untitled document with work in it is unsaved). */
export function documentHasContent(project: Project | null): boolean {
  if (project === null) return false;
  if (Object.keys(project.media).length > 0) return true;
  return Object.values(project.sequences).some((sequence) =>
    Object.values(sequence.tracks).some((track) => Object.keys(track.items).length > 0),
  );
}

/**
 * Whether leaving now would lose work: the engine says the project has unsaved changes
 * (`project.dirty`), or the document is untitled (never saved, not journaled) and has content.
 */
export function hasUnsavedWork(session: SessionState, project: Project | null): boolean {
  return session.dirty || (session.known && session.path === null && documentHasContent(project));
}

/** The file name of a project path, for titles and messages ("film.cutweave"). */
export function projectFileName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}
