// SPDX-License-Identifier: GPL-3.0-or-later
// The one place that changes which panels are docked (WP-2H.01): restoring a workspace's saved
// layout, closing a panel and remembering where it sat, reopening it there, and the live "which
// panels are open" state the View > Panels menu reads. `Dock` attaches its dockview API here;
// the `workspace.*` commands call these functions, so the tab's close button, the tab's menu, the
// View menu, the empty state and (later) keys and the command palette all take one path.
import type { DockviewApi } from "dockview-react";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import { applyDefaultLayout } from "../dock/defaultLayout.ts";
import { i18n } from "../i18n/index.ts";
import { toastStore } from "../state/toastStore.ts";
import { viewStore, type WorkspaceId } from "../state/viewStore.ts";
import {
  dockviewPart,
  EMPTY_META,
  type LayoutMeta,
  layoutProblem,
  type RememberedSpot,
  readLayoutMeta,
  type SavedLayout,
  spotInLayout,
  withLayoutMeta,
} from "./layoutMeta.ts";
import { panelOptions } from "./panelOptions.ts";
import { getPanelDef, type PanelDef, type PanelId, registeredPanels } from "./panelRegistry.ts";

/** What the menus and the empty state read: which panels of this workspace are open or closed. */
export interface OpenPanelsState {
  readonly open: readonly PanelId[];
  readonly closed: readonly PanelId[];
}

export const openPanelsStore = createStore<OpenPanelsState>()(() => ({ open: [], closed: [] }));

export function useOpenPanels(): OpenPanelsState {
  return useStore(openPanelsStore, (s) => s);
}

let api: DockviewApi | null = null;
let workspace: WorkspaceId | null = null;
let meta: LayoutMeta = EMPTY_META;

function refresh(): void {
  const open = api === null ? [] : api.panels.map((p) => p.id);
  const known = registeredPanels().map((p) => p.id);
  // A panel that starts closed (History) is not "closed" until someone opens and closes it: the
  // empty dock's "Reopen panels" brings back the panels the layout has, not every panel there is.
  const startClosed = new Set(
    registeredPanels()
      .filter((p) => p.startClosed)
      .map((p) => p.id),
  );
  openPanelsStore.setState({
    open: known.filter((id) => open.includes(id)),
    closed: known.filter((id) => !open.includes(id) && !startClosed.has(id)),
  });
}

/** The dock mounted (or was torn down: `null`). */
export function attachDock(next: DockviewApi | null): void {
  api = next;
  if (next === null) {
    workspace = null;
    meta = EMPTY_META;
  }
  refresh();
}

/** Docks `def` at `spot`; a reference panel that is not open means the dock's edge. */
function addAt(dock: DockviewApi, def: PanelDef, spot: RememberedSpot): void {
  const options = panelOptions(def);
  const reference = spot.ref === null ? undefined : dock.getPanel(spot.ref);
  const direction =
    spot.direction === "within" && reference === undefined ? "below" : spot.direction;
  const panel =
    dock.panels.length === 0
      ? dock.addPanel(options)
      : reference !== undefined
        ? dock.addPanel({ ...options, position: { referencePanel: reference, direction } })
        : dock.addPanel({
            ...options,
            position: { direction: direction === "within" ? "below" : direction },
          });
  if (spot.size !== undefined && spot.direction !== "within" && dock.width > 0 && dock.height > 0) {
    const horizontal = spot.direction === "left" || spot.direction === "right";
    panel.api.setSize(horizontal ? { width: spot.size } : { height: spot.size });
  }
  panel.api.setActive();
}

/** Writes the current layout, with the closed set, to the view store (and so to disk). */
export function saveLayoutNow(): void {
  if (api === null || workspace === null) return;
  viewStore.getState().saveLayout(workspace, withLayoutMeta(api.toJSON(), meta));
}

/**
 * Shows `saved` (or the default layout) for `id`. A saved layout that cannot be restored (a
 * corrupt blob, a panel nobody registers any more) is replaced by the default and a toast says
 * so. Returns whether it had to recover.
 */
export function loadWorkspace(dock: DockviewApi, id: WorkspaceId, saved: unknown): boolean {
  api = dock;
  workspace = id;
  const known = new Set(registeredPanels().map((p) => p.id));
  let problem: string | null = saved === undefined ? null : layoutProblem(saved, known);
  if (saved !== undefined && problem === null) {
    try {
      dock.fromJSON(dockviewPart(saved as SavedLayout));
      const loaded = readLayoutMeta(saved);
      meta = {
        ...loaded,
        closed: loaded.closed.filter((p) => known.has(p) && dock.getPanel(p) === undefined),
      };
      // A panel registered since this layout was saved (History, a plugin) appears at its default.
      for (const def of registeredPanels()) {
        if (
          dock.getPanel(def.id) === undefined &&
          def.startClosed !== true &&
          !meta.closed.includes(def.id)
        ) {
          addAt(dock, def, def.defaultDock);
        }
      }
      refresh();
      return false;
    } catch (error) {
      problem = error instanceof Error ? error.message : String(error);
    }
  }
  applyDefaultLayout(dock, id);
  meta = EMPTY_META;
  refresh();
  if (saved !== undefined) {
    toastStore.getState().push({
      tone: "warning",
      title: i18n.t("workspace.recovered.title"),
      message: i18n.t("workspace.recovered.body"),
    });
    console.warn(`workspace layout "${id}" could not be restored: ${problem ?? "unknown"}`);
  }
  return saved !== undefined;
}

/** The dock being shown (tests and the later palette's "float panel"); `null` before it mounts. */
export function currentDock(): DockviewApi | null {
  return api;
}

export function isPanelOpen(id: PanelId): boolean {
  return api?.getPanel(id) !== undefined;
}

/** Closes a panel and remembers where it was docked so reopening puts it back. */
export function closePanel(id: PanelId): void {
  const dock = api;
  const panel = dock?.getPanel(id);
  if (dock === null || panel === undefined) return;
  const spot = spotInLayout(dock.toJSON(), id);
  const { [id]: _old, ...rest } = meta.lastDock;
  meta = {
    ...meta,
    closed: [...meta.closed.filter((p) => p !== id), id],
    lastDock: spot === null ? rest : { ...rest, [id]: spot },
  };
  // The space it frees goes to the panel it was docked beside (the Preview widens when the
  // Inspector closes), instead of wherever the splitview happens to put it.
  const beside = spot !== null && spot.ref !== null ? dock.getPanel(spot.ref) : undefined;
  const horizontal = spot?.direction === "left" || spot?.direction === "right";
  const before = horizontal ? beside?.api.width : beside?.api.height;
  dock.removePanel(panel);
  if (beside !== undefined && spot?.size !== undefined && before !== undefined && before > 0) {
    beside.api.setSize(horizontal ? { width: before + spot.size } : { height: before + spot.size });
  }
  refresh();
  saveLayoutNow();
}

/** Opens a panel where it was last docked in this workspace, else at its default dock. */
export function openPanel(id: PanelId): void {
  const dock = api;
  const def = getPanelDef(id);
  if (dock === null || def === undefined) return;
  const existing = dock.getPanel(id);
  if (existing !== undefined) {
    existing.api.setActive();
    return;
  }
  const remembered = meta.lastDock[id];
  addAt(dock, def, remembered ?? def.defaultDock);
  const { [id]: _used, ...rest } = meta.lastDock;
  meta = { ...meta, closed: meta.closed.filter((p) => p !== id), lastDock: rest };
  refresh();
  saveLayoutNow();
}

export function togglePanel(id: PanelId): void {
  if (isPanelOpen(id)) closePanel(id);
  else openPanel(id);
}

/** Opens every closed panel of this workspace (the empty state's "Reopen panels"). */
export function reopenAllPanels(): void {
  for (const id of [...openPanelsStore.getState().closed]) openPanel(id);
}

/** Whether the panel's group fills the dock. */
export function isPanelMaximized(id: PanelId): boolean {
  return api?.getPanel(id)?.api.isMaximized() === true;
}

/** Maximizes the panel's group, or puts it back when it already fills the dock. */
export function toggleMaximizePanel(id: PanelId): void {
  const panel = api?.getPanel(id);
  if (panel === undefined) return;
  if (panel.api.isMaximized()) panel.api.exitMaximized();
  else panel.api.maximize();
}

/** Lifts the panel out of the layout into a floating window over it. */
export function floatPanel(id: PanelId): void {
  const dock = api;
  const panel = dock?.getPanel(id);
  if (dock === null || panel === undefined) return;
  dock.addFloatingGroup(panel);
  saveLayoutNow();
}

/** Docks the panel again at an edge of the dock: the left, the right or the bottom. */
export function movePanel(id: PanelId, side: "left" | "right" | "below"): void {
  const dock = api;
  const def = getPanelDef(id);
  const panel = dock?.getPanel(id);
  if (dock === null || def === undefined || panel === undefined) return;
  dock.removePanel(panel);
  addAt(dock, def, { ref: null, direction: side });
  refresh();
  saveLayoutNow();
}

/** Test seam: forget the attached dock. */
export function resetDockHost(): void {
  attachDock(null);
}
