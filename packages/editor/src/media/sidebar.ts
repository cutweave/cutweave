// SPDX-License-Identifier: GPL-3.0-or-later
// The bin's sidebar (WP-2F.06c, ADR-0036): the folder tree and the collections beside the grid.
// Here are its pure rules (which rows the tree shows, what an arrow key does, what may be dropped
// on a collection, when the panel is wide enough to show it beside the grid) and the part of its
// state that lasts for the session only (which folders are collapsed, which collection is being
// renamed, which one a drag is over). Whether it is shown and what the bin has open come back on
// the next launch: they are the bin store's. Nothing here is the project.
import type { CollectionId, FolderId, Project } from "@cutweave/protocol";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import { sessionStore } from "../session/sessionStore.ts";
import { docStore } from "../state/docStore.ts";
import { viewStore } from "../state/viewStore.ts";
import { binStore } from "./binStore.ts";
import { notYetOn, resolveOpenCollection } from "./collections.ts";
import { compareCodePoints, foldText } from "./find.ts";
import { type DragPayload, selectFolders } from "./folders.ts";
import { folderUiStore } from "./folderUi.ts";

/**
 * The narrowest media panel, in CSS px, that shows the sidebar beside the grid. Below it the
 * toggle opens the sidebar over the grid instead, and it closes again once something is opened.
 */
export const SIDEBAR_MIN_PANEL_WIDTH = 520;

export type SidebarMode = "inline" | "overlay";

/** How a panel `width` px wide shows the sidebar. A panel not measured yet (0) counts as wide. */
export const sidebarMode = (width: number): SidebarMode =>
  width === 0 || width >= SIDEBAR_MIN_PANEL_WIDTH ? "inline" : "overlay";

/** A row of the folder tree: "All media" (`id: null`) first, then the folders, depth first. */
export interface FolderRow {
  readonly id: FolderId | null;
  /** `null`: the top row, named by the panel. */
  readonly name: string | null;
  /** 1 for "All media" and the top-level folders, 2 for a folder inside one (`aria-level`). */
  readonly level: number;
  readonly parent: FolderId | null;
  readonly hasChildren: boolean;
  readonly expanded: boolean;
}

/**
 * The rows the tree shows: every folder whose ancestors are all expanded, siblings in name order
 * (ignoring case and accents). Folders are expanded unless they are in `collapsed`.
 */
export function folderRows(project: Project | null, collapsed: ReadonlySet<string>): FolderRow[] {
  const all = selectFolders(project);
  const byParent = new Map<FolderId | null, (typeof all)[number][]>();
  for (const folder of all) {
    const list = byParent.get(folder.parent);
    if (list === undefined) byParent.set(folder.parent, [folder]);
    else list.push(folder);
  }
  const rows: FolderRow[] = [
    {
      id: null,
      name: null,
      level: 1,
      parent: null,
      hasChildren: false,
      expanded: false,
    },
  ];
  const seen = new Set<string>();
  const walk = (parent: FolderId | null, level: number): void => {
    const kids = [...(byParent.get(parent) ?? [])].sort(
      (a, b) =>
        compareCodePoints(foldText(a.name), foldText(b.name)) ||
        compareCodePoints(a.name, b.name) ||
        compareCodePoints(a.id, b.id),
    );
    for (const folder of kids) {
      if (seen.has(folder.id)) continue;
      seen.add(folder.id);
      const hasChildren = (byParent.get(folder.id)?.length ?? 0) > 0;
      const expanded = hasChildren && !collapsed.has(folder.id);
      rows.push({
        id: folder.id,
        name: folder.name,
        level,
        parent: folder.parent,
        hasChildren,
        expanded,
      });
      if (expanded) walk(folder.id, level + 1);
    }
  };
  walk(null, 1);
  return rows;
}

/** What a key does in a tree. */
export type TreeMove =
  | { readonly kind: "focus"; readonly index: number }
  | { readonly kind: "expand" | "collapse"; readonly index: number }
  | null;

/** What the tree needs to know of a row to move through it. */
export interface TreeRowShape {
  readonly level: number;
  readonly hasChildren: boolean;
  readonly expanded: boolean;
}

/**
 * The tree keys of the ARIA pattern: Down and Up walk the visible rows; Right opens a closed row
 * or goes to its first child; Left closes an open row or goes to its parent; Home and End jump.
 */
export function treeMove(rows: readonly TreeRowShape[], index: number, key: string): TreeMove {
  const row = rows[index];
  if (row === undefined) return null;
  const last = rows.length - 1;
  switch (key) {
    case "ArrowDown":
      return index < last ? { kind: "focus", index: index + 1 } : null;
    case "ArrowUp":
      return index > 0 ? { kind: "focus", index: index - 1 } : null;
    case "Home":
      return index === 0 ? null : { kind: "focus", index: 0 };
    case "End":
      return index === last ? null : { kind: "focus", index: last };
    case "ArrowRight":
      if (!row.hasChildren) return null;
      return row.expanded ? { kind: "focus", index: index + 1 } : { kind: "expand", index };
    case "ArrowLeft": {
      if (row.hasChildren && row.expanded) return { kind: "collapse", index };
      for (let i = index - 1; i >= 0; i--) {
        if ((rows[i]?.level ?? 0) < row.level) return { kind: "focus", index: i };
      }
      return null;
    }
    default:
      return null;
  }
}

/**
 * Whether dropping `payload` on `collection` would add something: the collection is manual and at
 * least one dragged asset is not on it yet. Dragged folders are not added (a collection lists
 * assets).
 */
export function canDropOnCollection(
  project: Project | null,
  payload: DragPayload,
  collection: string,
): boolean {
  return notYetOn(project, collection, payload.media).length > 0;
}

export interface SidebarUiState {
  /** Folders whose children the tree hides. */
  readonly collapsed: ReadonlySet<string>;
  /** The collection whose name is being edited in place. */
  readonly renaming: CollectionId | null;
  /** The collection a drag is over and may drop on. */
  readonly dropOver: CollectionId | null;
  /** The sidebar is open over the grid (a narrow panel). */
  readonly overlay: boolean;
  setCollapsed(folder: string, collapsed: boolean): void;
  startRename(collection: CollectionId): void;
  stopRename(): void;
  setDropOver(collection: CollectionId | null): void;
  setOverlay(open: boolean): void;
}

export function createSidebarUiStore() {
  return createStore<SidebarUiState>()((set) => ({
    collapsed: new Set<string>(),
    renaming: null,
    dropOver: null,
    overlay: false,
    setCollapsed: (folder, collapsed) =>
      set((s) => {
        if (s.collapsed.has(folder) === collapsed) return s;
        const next = new Set(s.collapsed);
        if (collapsed) next.add(folder);
        else next.delete(folder);
        return { collapsed: next };
      }),
    startRename: (renaming) => set({ renaming }),
    stopRename: () => set((s) => (s.renaming === null ? s : { renaming: null })),
    setDropOver: (dropOver) => set((s) => (s.dropOver === dropOver ? s : { dropOver })),
    setOverlay: (overlay) => set((s) => (s.overlay === overlay ? s : { overlay })),
  }));
}

export const sidebarUiStore = createSidebarUiStore();

export function useSidebarUi<T>(selector: (state: SidebarUiState) => T): T {
  return useStore(sidebarUiStore, selector);
}

const projectKey = (): string => sessionStore.getState().path ?? "";

/** The collection the bin has open now (none when it is gone). */
export function currentOpenCollection(): CollectionId | null {
  return resolveOpenCollection(
    docStore.getState().project,
    binStore.getState().openCollections[projectKey()] ?? null,
  );
}

/**
 * Opens `collection` in the bin in place of a folder (`null`: back to the top level) and drops
 * the selection of the old place.
 */
export function openCollection(collection: CollectionId | null): void {
  if (collection === null) binStore.getState().openFolder(projectKey(), null);
  else binStore.getState().openCollection(projectKey(), collection);
  folderUiStore.getState().setFolders([], null);
  folderUiStore.getState().stopRename();
  const view = viewStore.getState();
  if (view.selection.some((r) => r.kind === "media")) {
    view.setSelection(view.selection.filter((r) => r.kind !== "media"));
  }
}

/** Shows or hides the sidebar: the panel's own choice, kept across launches. */
export function toggleSidebar(): void {
  const bin = binStore.getState();
  bin.setSidebar(!bin.sidebar);
}
