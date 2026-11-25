// SPDX-License-Identifier: GPL-3.0-or-later
// The bin's folder state that is not the document (WP-2F.08): which folders are selected (assets
// are selected in the view store, where the inspector and the timeline read them), which tile is
// being renamed, and which folder a drag is over. Session only; the open folder is the bin
// store's (it comes back with the project).
import type { FolderId, MediaId } from "@cutweave/protocol";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import { sessionStore } from "../session/sessionStore.ts";
import { docStore } from "../state/docStore.ts";
import { viewStore } from "../state/viewStore.ts";
import { binStore } from "./binStore.ts";
import { resolveOpenFolder, type Tile } from "./folders.ts";

export interface FolderUiState {
  /** Selected folders, in selection order. */
  readonly folders: readonly FolderId[];
  /** Where a Shift range starts. */
  readonly anchor: Tile | null;
  /** The tile whose name is being edited in place. */
  readonly renaming: Tile | null;
  /**
   * The drop target a drag is over: a folder, `null` for the top level, `undefined` for none.
   * Folder tiles and breadcrumb segments both show it.
   */
  readonly dropOver: FolderId | null | undefined;
  setFolders(folders: readonly FolderId[], anchor?: Tile | null): void;
  setAnchor(anchor: Tile | null): void;
  startRename(tile: Tile): void;
  stopRename(): void;
  setDropOver(target: FolderId | null | undefined): void;
}

export function createFolderUiStore() {
  return createStore<FolderUiState>()((set) => ({
    folders: [],
    anchor: null,
    renaming: null,
    dropOver: undefined,
    setFolders: (folders, anchor) =>
      set((s) => ({ folders, anchor: anchor === undefined ? s.anchor : anchor })),
    setAnchor: (anchor) => set({ anchor }),
    startRename: (renaming) => set({ renaming }),
    stopRename: () => set({ renaming: null }),
    setDropOver: (dropOver) => set((s) => (s.dropOver === dropOver ? s : { dropOver })),
  }));
}

export const folderUiStore = createFolderUiStore();

export function useFolderUi<T>(selector: (state: FolderUiState) => T): T {
  return useStore(folderUiStore, selector);
}

/** The key the bin keeps a project's open folder under: its file path ("" while untitled). */
export const projectKey = (): string => sessionStore.getState().path ?? "";

/** The folder the bin has open now (the top level when it is gone). */
export function currentOpenFolder(): FolderId | null {
  return resolveOpenFolder(
    docStore.getState().project,
    binStore.getState().openFolders[projectKey()] ?? null,
  );
}

/** Opens `folder` in the bin (`null`: the top level) and drops the selection of the old place. */
export function openFolder(folder: FolderId | null): void {
  binStore.getState().openFolder(projectKey(), folder);
  folderUiStore.getState().setFolders([], null);
  folderUiStore.getState().stopRename();
  const view = viewStore.getState();
  if (view.selection.some((r) => r.kind === "media")) {
    view.setSelection(view.selection.filter((r) => r.kind !== "media"));
  }
}

/** The assets and folders that are selected in the bin right now. */
export function selectedTiles(): { media: MediaId[]; folders: FolderId[] } {
  const media = viewStore
    .getState()
    .selection.filter((r) => r.kind === "media")
    .map((r) => r.id as MediaId);
  return { media, folders: [...folderUiStore.getState().folders] };
}
