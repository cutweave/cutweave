// SPDX-License-Identifier: GPL-3.0-or-later
// The bin's two editor-side actions that more than one place starts (WP-2F.08): New folder (the
// toolbar, Ctrl+Shift+N, the menus) and Select all. They live here, not in the menu tables, so the
// panel can call them without importing the menus. Each document change is a catalog command.
import type { FolderId } from "@cutweave/protocol";
import { docStore } from "../state/docStore.ts";
import { viewStore } from "../state/viewStore.ts";
import { createFolder } from "./binActions.ts";
import { binStore } from "./binStore.ts";
import { computeBinView } from "./binView.ts";
import { currentOpenFolder, folderUiStore, openFolder } from "./folderUi.ts";
import { binDeps } from "./requestDelete.ts";
import { currentOpenCollection } from "./sidebar.ts";

/**
 * Creates a folder in `parent` (`null`: the top level) and starts renaming it where it shows. A
 * search would hide the new folder, so it is cleared first; the folder's parent opens when it was
 * not the open one (the folder menu's "New folder").
 */
export async function newFolder(parent: FolderId | null): Promise<FolderId | null> {
  binStore.getState().clearAll();
  const id = await createFolder(binDeps(), parent);
  if (id === null) return null;
  // An open collection shows no folders: the new one's parent opens in its place.
  if (currentOpenCollection() !== null || parent !== currentOpenFolder()) openFolder(parent);
  viewStore.getState().setSelection([]);
  const tile = { kind: "folder", id } as const;
  folderUiStore.getState().setFolders([id], tile);
  folderUiStore.getState().startRename(tile);
  return id;
}

/** Selects every folder and asset the bin shows. */
export function selectAllInBin(): void {
  const bin = binStore.getState();
  const view = computeBinView(
    docStore.getState().project,
    { text: bin.text, filters: bin.filters, sort: bin.sort, scope: bin.scope },
    currentOpenFolder(),
    currentOpenCollection(),
  );
  folderUiStore.getState().setFolders(
    view.folders.map((f) => f.id),
    view.order[0] ?? null,
  );
  viewStore.getState().setSelection(view.cards.map((c) => ({ kind: "media" as const, id: c.id })));
}
