// SPDX-License-Identifier: GPL-3.0-or-later
// The one place that changes which panels are docked (WP-2H.01): restoring a workspace's saved
// layout, closing a panel and remembering where it sat, reopening it there, and the live "which
// panels are open" state the View > Panels menu reads. `Dock` attaches its dockview API here;
// the `workspace.*` commands call these functions, so the tab's close button, the tab's menu, the
// View menu, the empty state and (later) keys and the command palette all take one path.
//
// Detached panels (WP-2H.14, ADR-0031): a panel moves into its own OS window as a dockview popout
// group (`popoutPanel`), from the tab's menu, the palette or a tab dragged out of the window, and
// back (`dockPanel`, or closing that window). Its window is registered in `shell/windows.ts`, and
// the popouts are part of the workspace's saved layout, so a relaunch reopens them.
import type { DockviewApi, IDockviewPanel, TabDragEvent } from "dockview-react";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import { applyDefaultLayout } from "../dock/defaultLayout.ts";
import { i18n, type TKey } from "../i18n/index.ts";
import { addPopoutWindow, registeredWindows, removePopoutWindow } from "../shell/windows.ts";
import { toastStore } from "../state/toastStore.ts";
import { viewStore, type WorkspaceId } from "../state/viewStore.ts";
import { dragOutPoint, screenRectOf } from "./dragOut.ts";
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

/** The page a detached panel's window loads: main allows this one and nothing else (ADR-0031). */
export const POPOUT_URL = "/popout.html";

/** Listeners on the attached dock (popouts, tab drags), dropped when it detaches. */
let wiring: { dispose(): void }[] = [];
/**
 * The main window is going away: the dock closes its popout windows, which puts their panels back
 * in the grid. That is not the person's layout, so it is not saved (the relaunch reopens them).
 */
let unloading = false;
/** A loaded layout's popout windows are still opening: until they are, the layout is partial. */
let restoring = false;
let restoreGeneration = 0;
/** How long a layout's popout windows may take to open before saving resumes regardless. */
const RESTORE_TIMEOUT_MS = 10_000;

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
  for (const disposable of wiring.splice(0)) disposable.dispose();
  api = next;
  if (next === null) {
    workspace = null;
    meta = EMPTY_META;
    restoring = false;
    syncPopoutWindows(null);
  } else {
    wiring = wirePopouts(next);
  }
  refresh();
}

/** The panels in the window `win` (a detached window, or the main one), in tab order. */
function panelsIn(dock: DockviewApi, win: Window): IDockviewPanel[] {
  return dock.panels.filter((p) => p.api.location.type === "popout" && safeWindow(p) === win);
}

function safeWindow(panel: IDockviewPanel): Window | null {
  try {
    return panel.api.getWindow();
  } catch {
    return null;
  }
}

/** Registers each detached window with the panels it holds, and forgets the closed ones. */
function syncPopoutWindows(dock: DockviewApi | null): void {
  const held = new Map<Window, TKey[]>();
  for (const panel of dock?.panels ?? []) {
    if (panel.api.location.type !== "popout") continue;
    const win = safeWindow(panel);
    const title = getPanelDef(panel.id)?.title;
    if (win === null || title === undefined) continue;
    held.set(win, [...(held.get(win) ?? []), title]);
  }
  for (const entry of registeredWindows()) {
    if (entry.role === "popout" && !held.has(entry.window)) removePopoutWindow(entry.window);
  }
  // Registers a new window, or retitles one whose panels changed.
  for (const [win, titles] of held) addPopoutWindow(win, titles);
}

/** Listens to the dock for detached windows: open, close, fail, and tabs dragged out. */
function wirePopouts(dock: DockviewApi): { dispose(): void }[] {
  const sync = () => syncPopoutWindows(dock);
  const onUnload = () => {
    unloading = true;
  };
  // Registered before any popout opens, so it runs before the dock's own `beforeunload`.
  globalThis.addEventListener?.("beforeunload", onUnload);
  return [
    dock.onDidAddPopoutGroup(sync),
    dock.onDidRemovePopoutGroup(sync),
    dock.onDidLayoutChange(sync),
    dock.onDidOpenPopoutWindowFail(() => {
      toastStore.getState().push({
        tone: "warning",
        title: i18n.t("windows.popoutFailed.title"),
        message: i18n.t("windows.popoutFailed.body"),
      });
    }),
    dock.onWillDragPanel((event) => watchDragOut(dock, event)),
    { dispose: () => globalThis.removeEventListener?.("beforeunload", onUnload) },
  ];
}

/**
 * A tab drag started: if it ends with no drop and the pointer outside every editor window, the
 * panel opens in its own window there. Mouse drags only (touch and pen drags have no `dragend`).
 */
function watchDragOut(dock: DockviewApi, event: TabDragEvent): void {
  const native = event.nativeEvent;
  if (!("dataTransfer" in native)) return;
  const source = native.target;
  if (source === null || typeof (source as EventTarget).addEventListener !== "function") return;
  const id = event.panel.id;
  const from = safeWindow(event.panel);
  (source as EventTarget).addEventListener(
    "dragend",
    (end) => {
      const point = dragOutPoint(
        end as DragEvent,
        registeredWindows().map((w) => screenRectOf(w.window)),
      );
      if (point === null || api !== dock) return;
      // Alone in its own window already: dragging its tab out moves nothing.
      if (isPanelDetached(id) && from !== null && panelsIn(dock, from).length === 1) return;
      void popoutPanel(id, point);
    },
    { once: true },
  );
}

/** Docks `def` at `spot`; a reference panel that is not open means the dock's edge. */
function addAt(dock: DockviewApi, def: PanelDef, spot: RememberedSpot): void {
  const options = panelOptions(def);
  // A reference in a detached window is not a spot in the main window's grid.
  const found = spot.ref === null ? undefined : dock.getPanel(spot.ref);
  const reference = found?.api.location.type === "popout" ? undefined : found;
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

/**
 * Writes the current layout, with the closed set, to the view store (and so to disk). Not while
 * the window unloads or a loaded layout's detached windows are still opening: either would save a
 * layout that lost them.
 */
export function saveLayoutNow(): void {
  if (api === null || workspace === null || unloading || restoring) return;
  viewStore.getState().saveLayout(workspace, withLayoutMeta(api.toJSON(), meta));
}

/** A loaded layout has detached windows: hold the saves until they are open (or failed). */
function awaitPopoutRestore(dock: DockviewApi, saved: SavedLayout): void {
  if ((saved.popoutGroups?.length ?? 0) === 0) {
    restoring = false;
    return;
  }
  restoring = true;
  const generation = ++restoreGeneration;
  const done = () => {
    if (generation !== restoreGeneration || api !== dock || !restoring) return;
    restoring = false;
    refresh();
    syncPopoutWindows(dock);
    saveLayoutNow();
  };
  void dock.popoutRestorationPromise.then(done, done);
  setTimeout(done, RESTORE_TIMEOUT_MS);
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
  restoreGeneration += 1;
  restoring = false;
  if (saved !== undefined && problem === null) {
    try {
      dock.fromJSON(dockviewPart(saved as SavedLayout));
      awaitPopoutRestore(dock, saved as SavedLayout);
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
  // A detached panel is not in the grid: it reopens where it sat before it left the window.
  const remembered = spot ?? (isPanelDetached(id) ? meta.lastDock[id] : undefined) ?? null;
  const { [id]: _old, ...rest } = meta.lastDock;
  meta = {
    ...meta,
    closed: [...meta.closed.filter((p) => p !== id), id],
    lastDock: remembered === null ? rest : { ...rest, [id]: remembered },
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

/** Whether the panel is in a window of its own (a detached panel, ADR-0031). */
export function isPanelDetached(id: PanelId): boolean {
  return api?.getPanel(id)?.api.location.type === "popout";
}

/** The panels in windows of their own. */
export function detachedPanels(): PanelId[] {
  return (api?.panels ?? []).filter((p) => p.api.location.type === "popout").map((p) => p.id);
}

/**
 * Moves the panel into a window of its own: the size it has now, at `at` (screen pixels: where a
 * dragged tab was let go) or over where it is. Where it sat is remembered, so it can come back.
 * Resolves to whether the window opened; when it did not, the panel stays where it is and a
 * notice says so (`onDidOpenPopoutWindowFail`).
 */
export async function popoutPanel(id: PanelId, at?: { x: number; y: number }): Promise<boolean> {
  const dock = api;
  const panel = dock?.getPanel(id);
  if (dock === null || panel === undefined) return false;
  if (panel.api.location.type === "popout") {
    safeWindow(panel)?.focus();
    return true;
  }
  const spot = spotInLayout(dock.toJSON(), id);
  if (spot !== null) meta = { ...meta, lastDock: { ...meta.lastDock, [id]: spot } };
  const group = panel.group;
  const position =
    at === undefined
      ? undefined
      : {
          left: at.x,
          top: at.y,
          width: Math.round(group.api.width),
          height: Math.round(group.api.height),
        };
  const opened = await dock.addPopoutGroup(panel, {
    popoutUrl: POPOUT_URL,
    ...(position === undefined ? {} : { position }),
  });
  if (api !== dock) return opened;
  refresh();
  syncPopoutWindows(dock);
  saveLayoutNow();
  return opened;
}

/**
 * Moves a detached panel back into the main window. Alone in its window, the window closes and
 * the dock puts the panel back where it was (as when the person closes that window); sharing one,
 * it docks at the spot it left, and the window keeps the other panels.
 */
export function dockPanel(id: PanelId): void {
  const dock = api;
  const def = getPanelDef(id);
  const panel = dock?.getPanel(id);
  if (dock === null || def === undefined || panel === undefined) return;
  if (panel.api.location.type !== "popout") return;
  const win = safeWindow(panel);
  if (win !== null && panelsIn(dock, win).length === 1) {
    win.close();
    return;
  }
  dock.removePanel(panel);
  addAt(dock, def, meta.lastDock[id] ?? def.defaultDock);
  const { [id]: _used, ...rest } = meta.lastDock;
  meta = { ...meta, lastDock: rest };
  refresh();
  syncPopoutWindows(dock);
  saveLayoutNow();
}

/** The panel the person is working in: the dock's active one. */
export function activePanelId(): PanelId | undefined {
  return api?.activePanel?.id;
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
  unloading = false;
}
