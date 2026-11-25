// SPDX-License-Identifier: GPL-3.0-or-later
// What a saved workspace layout carries besides dockview's own JSON (WP-2H.01): the panels the
// person closed and the spot each one was last docked at. It rides inside the layout object
// (`cutweave` key, which dockview ignores), so it is saved, mirrored to the engine's
// `panel_layout` and reset together with the layout, and a closed panel stays closed per
// workspace across a relaunch. Pure functions; unit-tested without a DOM.
import type { SerializedDockview } from "dockview-react";
import type { DockSpot, PanelId } from "./panelRegistry.ts";

/** Bump when the `cutweave` block changes shape; an unknown version reads as "no metadata". */
export const LAYOUT_META_VERSION = 1;

/** A remembered dock: where it sat, and how big (px along the axis it was split on). */
export interface RememberedSpot extends DockSpot {
  readonly size?: number;
}

export interface LayoutMeta {
  readonly v: typeof LAYOUT_META_VERSION;
  /** Panels the person closed in this workspace. */
  readonly closed: readonly PanelId[];
  /** Where each closed panel was last docked. */
  readonly lastDock: Readonly<Record<PanelId, RememberedSpot>>;
}

export const EMPTY_META: LayoutMeta = { v: LAYOUT_META_VERSION, closed: [], lastDock: {} };

/** A layout as stored: dockview's JSON plus our block. */
export type SavedLayout = SerializedDockview & { cutweave?: unknown };

const DIRECTIONS: readonly string[] = ["left", "right", "above", "below", "within"];

function sanitizeSpot(value: unknown): RememberedSpot | null {
  if (typeof value !== "object" || value === null) return null;
  const s = value as Record<string, unknown>;
  if (typeof s.direction !== "string" || !DIRECTIONS.includes(s.direction)) return null;
  if (s.ref !== null && typeof s.ref !== "string") return null;
  const spot: RememberedSpot = {
    ref: s.ref,
    direction: s.direction as DockSpot["direction"],
    ...(typeof s.size === "number" && Number.isFinite(s.size) && s.size > 0
      ? { size: s.size }
      : {}),
  };
  return spot;
}

/** The metadata of a saved layout; empty when it has none or it is from another version. */
export function readLayoutMeta(layout: unknown): LayoutMeta {
  if (typeof layout !== "object" || layout === null) return EMPTY_META;
  const raw = (layout as { cutweave?: unknown }).cutweave;
  if (typeof raw !== "object" || raw === null) return EMPTY_META;
  const m = raw as Record<string, unknown>;
  if (m.v !== LAYOUT_META_VERSION) return EMPTY_META;
  const closed = Array.isArray(m.closed)
    ? [...new Set(m.closed.filter((id): id is string => typeof id === "string"))]
    : [];
  const lastDock: Record<PanelId, RememberedSpot> = {};
  if (typeof m.lastDock === "object" && m.lastDock !== null) {
    for (const [id, value] of Object.entries(m.lastDock)) {
      const spot = sanitizeSpot(value);
      if (spot !== null) lastDock[id] = spot;
    }
  }
  return { v: LAYOUT_META_VERSION, closed, lastDock };
}

/** `layout` with `meta` attached (and without the block when there is nothing to remember). */
export function withLayoutMeta(layout: SerializedDockview, meta: LayoutMeta): SavedLayout {
  const { cutweave: _old, ...rest } = layout as SavedLayout;
  if (meta.closed.length === 0 && Object.keys(meta.lastDock).length === 0) return rest;
  return { ...rest, cutweave: meta };
}

/** `layout` without our block, ready for `DockviewApi.fromJSON`. */
export function dockviewPart(layout: SavedLayout): SerializedDockview {
  const { cutweave: _meta, ...rest } = layout;
  return rest;
}

interface GridNode {
  type?: string;
  data?: unknown;
  size?: number;
}

const isRecord = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null;

function leafViews(node: GridNode): string[] {
  const data = node.data;
  if (!isRecord(data) || !Array.isArray(data.views)) return [];
  return data.views.filter((v): v is string => typeof v === "string");
}

function branchChildren(node: GridNode): GridNode[] {
  return Array.isArray(node.data) ? (node.data as GridNode[]) : [];
}

function firstPanel(node: GridNode): PanelId | null {
  if (node.type === "leaf") return leafViews(node)[0] ?? null;
  for (const child of branchChildren(node)) {
    const found = firstPanel(child);
    if (found !== null) return found;
  }
  return null;
}

/**
 * Why a saved layout cannot be restored (`null` when it can): not an object, no grid, a panel in
 * the grid with no entry, or a panel nobody registered (a plugin removed since). The caller falls
 * back to the default layout and says so, rather than letting dockview throw halfway.
 */
export function layoutProblem(layout: unknown, known: ReadonlySet<PanelId>): string | null {
  if (!isRecord(layout)) return "not an object";
  const grid = layout.grid;
  if (!isRecord(grid) || !isRecord(grid.root)) return "no grid";
  if (grid.orientation !== "HORIZONTAL" && grid.orientation !== "VERTICAL") {
    return "bad orientation";
  }
  const panels = layout.panels;
  if (!isRecord(panels)) return "no panels";
  for (const id of Object.keys(panels)) {
    if (!known.has(id)) return `unknown panel ${id}`;
  }
  const missing = (node: GridNode): string | null => {
    if (node.type === "leaf") {
      for (const view of leafViews(node)) if (!(view in panels)) return `no state for ${view}`;
      return null;
    }
    if (node.type !== "branch") return "bad grid node";
    for (const child of branchChildren(node)) {
      const problem = missing(child);
      if (problem !== null) return problem;
    }
    return null;
  };
  return missing(grid.root as GridNode);
}

/**
 * Where panel `id` sits in a layout's grid, as a spot to dock it back at: beside the neighbour it
 * shares a split with (or tabbed with, `within`), plus the size it had. `null` when it is not in
 * the grid (floating, or absent), and the caller uses the panel's default dock.
 */
export function spotInLayout(layout: SerializedDockview, id: PanelId): RememberedSpot | null {
  const flip = (o: string): string => (o === "HORIZONTAL" ? "VERTICAL" : "HORIZONTAL");
  const find = (node: GridNode, orientation: string, isRoot: boolean): RememberedSpot | null => {
    const children = branchChildren(node);
    for (let i = 0; i < children.length; i++) {
      const child = children[i];
      if (child === undefined) continue;
      if (child.type === "leaf" && leafViews(child).includes(id)) {
        const views = leafViews(child);
        if (views.length > 1) {
          const other = views.find((v) => v !== id);
          return other === undefined ? null : { ref: other, direction: "within" };
        }
        const after = children[i + 1];
        const sibling = after ?? children[i - 1];
        if (sibling === undefined) return null;
        const horizontal = orientation === "HORIZONTAL";
        const direction: DockSpot["direction"] = horizontal
          ? after !== undefined
            ? "left"
            : "right"
          : after !== undefined
            ? "above"
            : "below";
        const ref = isRoot && sibling.type === "branch" ? null : firstPanel(sibling);
        return {
          ref,
          direction,
          ...(typeof child.size === "number" && child.size > 0 ? { size: child.size } : {}),
        };
      }
      if (child.type === "branch") {
        const deeper = find(child, flip(orientation), false);
        if (deeper !== null) return deeper;
      }
    }
    return null;
  };
  const grid = layout.grid;
  if (!grid?.root) return null;
  return find(grid.root as unknown as GridNode, grid.orientation, true);
}
