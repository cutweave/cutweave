// SPDX-License-Identifier: GPL-3.0-or-later
// The bin's folders as the editor reads them (WP-2F.08, ADR-0020): the tree from the document
// mirror, what a folder holds, the breadcrumb, which folders a search covers, and the pure rules
// for dragging tiles onto folders. Every change goes through `media.*` commands (binActions.ts);
// nothing here writes. Folders are addressed by ID, never by position (ADR-0004).
import type { FolderId, Project } from "@cutweave/protocol";
import type { MediaCard } from "./cards.ts";
import { compareCodePoints, foldText } from "./find.ts";
import { folderSubtree } from "./folderScope.ts";

/** One folder of the project. */
export interface FolderEntry {
  readonly id: FolderId;
  readonly name: string;
  readonly parent: FolderId | null;
}

const entryCache = new WeakMap<Project, readonly FolderEntry[]>();

/** The project's folders, by ID. */
export function selectFolders(project: Project | null): readonly FolderEntry[] {
  if (project === null) return [];
  let entries = entryCache.get(project);
  if (entries === undefined) {
    entries = Object.keys(project.folders ?? {})
      .sort()
      .flatMap((key) => {
        const folder = project.folders[key as FolderId];
        return folder === undefined
          ? []
          : [{ id: folder.id, name: folder.name, parent: folder.parent ?? null }];
      });
    entryCache.set(project, entries);
  }
  return entries;
}

/** Names from the top level down to `folder` (empty for the top level or an unknown folder). */
export function folderNames(project: Project | null, folder: FolderId | null): string[] {
  const byId = new Map(selectFolders(project).map((f) => [f.id, f]));
  const names: string[] = [];
  const seen = new Set<string>();
  for (let at = folder === null ? undefined : byId.get(folder); at !== undefined; ) {
    if (seen.has(at.id)) break;
    seen.add(at.id);
    names.unshift(at.name);
    at = at.parent === null ? undefined : byId.get(at.parent);
  }
  return names;
}

/** "Interviews / Day 1": where a folder is, as the search results caption it. */
export function folderPathText(project: Project | null, folder: FolderId | null): string {
  return folderNames(project, folder).join(" / ");
}

/** The open folder if it still exists, else the top level (it was deleted, or another project opened). */
export function resolveOpenFolder(project: Project | null, open: FolderId | null): FolderId | null {
  if (open === null || project === null) return null;
  return project.folders?.[open] === undefined ? null : open;
}

/** A segment of the breadcrumb: the top level (`id: null`), then each folder down to the open one. */
export interface Crumb {
  readonly id: FolderId | null;
  readonly name: string | null;
}

export function breadcrumbOf(project: Project | null, open: FolderId | null): Crumb[] {
  const byId = new Map(selectFolders(project).map((f) => [f.id, f]));
  const chain: Crumb[] = [];
  const seen = new Set<string>();
  for (let at = open === null ? undefined : byId.get(open); at !== undefined; ) {
    if (seen.has(at.id)) break;
    seen.add(at.id);
    chain.unshift({ id: at.id, name: at.name });
    at = at.parent === null ? undefined : byId.get(at.parent);
  }
  return [{ id: null, name: null }, ...chain];
}

/** A folder as the grid shows it. */
export interface FolderTileData extends FolderEntry {
  /** Assets directly inside. */
  readonly media: number;
  /** Folders directly inside. */
  readonly folders: number;
  /** What the tile counts: assets and folders directly inside. */
  readonly items: number;
}

/** Name order of two folders: ignoring case and accents, then by code point, then ID. */
const byName = (a: FolderEntry, b: FolderEntry): number =>
  compareCodePoints(foldText(a.name), foldText(b.name)) ||
  compareCodePoints(a.name, b.name) ||
  compareCodePoints(a.id, b.id);

/** How many assets and folders each folder holds directly. */
export function folderCounts(
  project: Project | null,
  cards: readonly MediaCard[],
): ReadonlyMap<FolderId, { media: number; folders: number }> {
  const counts = new Map<FolderId, { media: number; folders: number }>();
  const at = (id: FolderId) => {
    let c = counts.get(id);
    if (c === undefined) {
      c = { media: 0, folders: 0 };
      counts.set(id, c);
    }
    return c;
  };
  for (const folder of selectFolders(project)) {
    at(folder.id);
    if (folder.parent !== null) at(folder.parent).folders += 1;
  }
  for (const card of cards) if (card.folder !== null) at(card.folder).media += 1;
  return counts;
}

/** The folders directly inside `parent` (`null`: the top level), in name order, with their counts. */
export function childFolders(
  project: Project | null,
  cards: readonly MediaCard[],
  parent: FolderId | null,
): FolderTileData[] {
  const counts = folderCounts(project, cards);
  return selectFolders(project)
    .filter((f) => f.parent === parent)
    .sort(byName)
    .map((f) => {
      const c = counts.get(f.id) ?? { media: 0, folders: 0 };
      return { ...f, media: c.media, folders: c.folders, items: c.media + c.folders };
    });
}

/** Folders whose name contains every term of the search, in name order (the "All media" results). */
export function matchingFolders(
  project: Project | null,
  cards: readonly MediaCard[],
  terms: readonly string[],
): FolderTileData[] {
  if (terms.length === 0) return [];
  const counts = folderCounts(project, cards);
  return selectFolders(project)
    .filter((f) => {
      const name = foldText(f.name);
      return terms.every((t) => name.includes(t));
    })
    .sort(byName)
    .map((f) => {
      const c = counts.get(f.id) ?? { media: 0, folders: 0 };
      return { ...f, media: c.media, folders: c.folders, items: c.media + c.folders };
    });
}

// ---- the selection model ------------------------------------------------------------------

/** One tile of the grid: a folder or an asset. */
export interface Tile {
  readonly kind: "folder" | "media";
  readonly id: string;
}

export interface TileSelection {
  readonly tiles: readonly Tile[];
  /** Where a Shift range starts; the last tile clicked without Shift. */
  readonly anchor: Tile | null;
}

export const NO_TILES: TileSelection = { tiles: [], anchor: null };

const same = (a: Tile, b: Tile): boolean => a.kind === b.kind && a.id === b.id;
const indexOfTile = (order: readonly Tile[], tile: Tile): number =>
  order.findIndex((t) => same(t, tile));

export interface ClickMods {
  readonly ctrl: boolean;
  readonly shift: boolean;
}

/**
 * What a click on `tile` does to the selection. `order` is the grid as drawn (folders, then
 * assets), so a Shift range runs across both. Plain: only this tile. Ctrl or Cmd: toggles it and
 * moves the anchor. Shift: the range from the anchor to it (added to the selection with Ctrl).
 */
export function clickTile(
  order: readonly Tile[],
  current: TileSelection,
  tile: Tile,
  mods: ClickMods,
): TileSelection {
  if (mods.shift) {
    const from = current.anchor === null ? -1 : indexOfTile(order, current.anchor);
    const to = indexOfTile(order, tile);
    if (to < 0) return current;
    const start = from < 0 ? to : from;
    const range = order.slice(Math.min(start, to), Math.max(start, to) + 1);
    const base = mods.ctrl ? current.tiles : [];
    const merged = [...base, ...range.filter((t) => !base.some((b) => same(b, t)))];
    return { tiles: merged, anchor: from < 0 ? tile : (current.anchor as Tile) };
  }
  if (mods.ctrl) {
    const has = current.tiles.some((t) => same(t, tile));
    return {
      tiles: has ? current.tiles.filter((t) => !same(t, tile)) : [...current.tiles, tile],
      anchor: tile,
    };
  }
  return { tiles: [tile], anchor: tile };
}

/** Ctrl or Cmd+A: every tile of the grid. */
export function selectAllTiles(order: readonly Tile[]): TileSelection {
  return { tiles: [...order], anchor: order[0] ?? null };
}

/** The selection without tiles that are no longer on the grid (deleted, moved away, filtered out). */
export function pruneSelection(order: readonly Tile[], current: TileSelection): TileSelection {
  const tiles = current.tiles.filter((t) => indexOfTile(order, t) >= 0);
  if (tiles.length === current.tiles.length) return current;
  const anchor =
    current.anchor !== null && indexOfTile(order, current.anchor) >= 0 ? current.anchor : null;
  return { tiles, anchor };
}

// ---- dragging tiles onto folders ----------------------------------------------------------

/** What a drag carries: the selected assets and folders. */
export interface DragPayload {
  readonly media: readonly string[];
  readonly folders: readonly string[];
}

/** The two things `resolveDropTarget` reads of an element, so tests need no DOM. */
export interface DropElement {
  getAttribute(name: string): string | null;
  readonly parentElement: DropElement | null;
}

/** The attribute that marks a drop target: a folder ID, or "" for the top level. */
export const DROP_ATTRIBUTE = "data-drop-folder";

/**
 * The folder a drop on `element` goes to: the closest ancestor (or itself) that is a folder tile
 * or a breadcrumb segment. `{folder: null}` is the top level; `null` is no target at all.
 */
export function resolveDropTarget(element: DropElement | null): { folder: FolderId | null } | null {
  for (let at = element; at !== null; at = at.parentElement) {
    const value = at.getAttribute(DROP_ATTRIBUTE);
    if (value !== null) return { folder: value === "" ? null : (value as FolderId) };
  }
  return null;
}

/**
 * Whether dropping `payload` on `to` would do something the engine accepts: no folder goes into
 * itself or below itself, and at least one asset or folder is somewhere else now.
 */
export function canDrop(
  project: Project | null,
  payload: DragPayload,
  to: FolderId | null,
): boolean {
  if (project === null) return false;
  if (to !== null && project.folders?.[to] === undefined) return false;
  const folders = project.folders ?? {};
  for (const id of payload.folders) {
    if (to !== null && folderSubtree(folders, id).has(to)) return false;
  }
  const movesMedia = payload.media.some(
    (id) => (project.media[id as never]?.folder ?? null) !== to,
  );
  const movesFolders = payload.folders.some(
    (id) => (folders[id as FolderId]?.parent ?? null) !== to,
  );
  return movesMedia || movesFolders;
}

// ---- deleting a folder --------------------------------------------------------------------

/** What deleting a folder would affect, for the confirmation. */
export interface FolderDeletePlan {
  readonly folder: FolderId;
  readonly name: string;
  /** The folder its kept contents go to; `null`: the top level. */
  readonly parent: FolderId | null;
  readonly parentName: string | null;
  /** Assets inside it and below. */
  readonly media: number;
  /** Of those, the ones a timeline uses (they move up with `remove`). */
  readonly used: number;
  /** Folders below it. */
  readonly subfolders: number;
  /** Nothing inside: it deletes without asking. */
  readonly empty: boolean;
}

export function planFolderDelete(
  project: Project | null,
  cards: readonly MediaCard[],
  usage: ReadonlyMap<string, number>,
  folder: FolderId,
): FolderDeletePlan | null {
  const entry = selectFolders(project).find((f) => f.id === folder);
  if (project === null || entry === undefined) return null;
  const subtree = folderSubtree(project.folders, folder);
  const inside = cards.filter((c) => c.folder !== null && subtree.has(c.folder));
  const used = inside.filter((c) => (usage.get(c.id) ?? 0) > 0).length;
  const parent = entry.parent;
  return {
    folder,
    name: entry.name,
    parent,
    parentName: parent === null ? null : (project.folders[parent]?.name ?? null),
    media: inside.length,
    used,
    subfolders: subtree.size - 1,
    empty: inside.length === 0 && subtree.size === 1,
  };
}
