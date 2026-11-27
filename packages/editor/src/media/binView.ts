// SPDX-License-Identifier: GPL-3.0-or-later
// What the bin shows (WP-2F.08): the open folder's subfolders and assets, or, while searching or
// filtering, results across the whole bin ("All media", flat, folders whose names match first) or
// inside the open folder and below ("This folder"). One pure function the panel, the keyboard and
// the menu commands all read, so "select all" and a click agree on what is on the grid.
import type { CollectionId, FolderId, MediaSort, Project } from "@cutweave/protocol";
import type { BinFilters, SearchScope } from "./binStore.ts";
import { toMediaFilter } from "./binStore.ts";
import { type MediaCard, selectMediaCards, selectMediaUsage } from "./cards.ts";
import { collectionCards } from "./collections.ts";
import { findMedia, termsOf } from "./find.ts";
import { folderSubtree } from "./folderScope.ts";
import { childFolders, type FolderTileData, matchingFolders, type Tile } from "./folders.ts";

export interface BinViewInput {
  readonly text: string;
  readonly filters: BinFilters;
  readonly sort: Required<MediaSort>;
  readonly scope: SearchScope;
}

export interface BinView {
  /** The folder the bin has open (`null`: the top level, or a collection is open). */
  readonly open: FolderId | null;
  /** The collection the bin has open in place of a folder (WP-2F.06c); `null`: none. */
  readonly collection: CollectionId | null;
  /** A search text or a filter is on: results are flat and carry their folder path. */
  readonly searching: boolean;
  /** Folder tiles, before the assets. */
  readonly folders: readonly FolderTileData[];
  /** Asset tiles in the requested order. */
  readonly cards: readonly MediaCard[];
  /** Every asset of the project (for "is the bin empty at all"). */
  readonly all: readonly MediaCard[];
  /** The grid as drawn, folders first: the order a Shift range and the arrow keys walk. */
  readonly order: readonly Tile[];
}

export function computeBinView(
  project: Project | null,
  input: BinViewInput,
  open: FolderId | null,
  collection: CollectionId | null = null,
): BinView {
  const all = selectMediaCards(project);
  const usage = selectMediaUsage(project);
  const base = toMediaFilter({ text: input.text, filters: input.filters });
  const terms = termsOf(input.text);
  const searching = terms.length > 0 || Object.keys(base).some((k) => k !== "text");
  const folders = project?.folders ?? {};

  let found: readonly MediaCard[];
  let tiles: readonly FolderTileData[];
  if (collection !== null) {
    // An open collection: its members from every folder, flat, narrowed by the search and the
    // filters. No folder tiles: a collection lists assets.
    found = collectionCards(project, all, usage, collection, base, input.sort);
    tiles = [];
  } else if (!searching) {
    // Browsing: one level of the open folder.
    found = findMedia(
      all,
      usage,
      { ...base, folder: open, recursive: false },
      input.sort,
      folders,
    ).cards;
    tiles = childFolders(project, all, open);
  } else if (input.scope === "folder" && open !== null) {
    found = findMedia(
      all,
      usage,
      { ...base, folder: open, recursive: true },
      input.sort,
      folders,
    ).cards;
    const inside = folderSubtree(folders, open);
    inside.delete(open);
    tiles = matchingFolders(project, all, terms).filter((f) => inside.has(f.id));
  } else {
    found = findMedia(all, usage, base, input.sort, folders).cards;
    tiles = matchingFolders(project, all, terms);
  }
  return {
    open: collection === null ? open : null,
    collection,
    searching,
    folders: tiles,
    cards: found,
    all,
    order: [
      ...tiles.map((f): Tile => ({ kind: "folder", id: f.id })),
      ...found.map((c): Tile => ({ kind: "media", id: c.id })),
    ],
  };
}
