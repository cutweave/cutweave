// SPDX-License-Identifier: GPL-3.0-or-later
import type { MediaId, SelectionRef, SourceMarks, UiState } from "@cutweave/protocol";
import type { SerializedDockview } from "dockview-react";
import { useStore } from "zustand";
import { createJSONStorage, persist, type StateStorage } from "zustand/middleware";
import { createStore } from "zustand/vanilla";
import type { TKey } from "../i18n/index.ts";

// The theme and the saving preferences are app preferences, not view state: they live in the
// preferences store (src/prefs, WP-1M.11). The persisted blob this store wrote before that may
// still hold them; `migrateLegacyView` (src/prefs/legacy.ts) moves them once and removes them.

export const WORKSPACES = ["short-form", "long-form"] as const;
export type WorkspaceId = (typeof WORKSPACES)[number];

export const ASSET_TABS = [
  "media",
  "audio",
  "text",
  "stickers",
  "effects",
  "transitions",
  "filters",
  "templates",
  "captions",
  "ai-studio",
] as const;
export type AssetTabId = (typeof ASSET_TABS)[number];

export interface AspectDef {
  id: string;
  width: number;
  height: number;
  labelKey: TKey;
}

export const ASPECTS = [
  { id: "9:16", width: 9, height: 16, labelKey: "preview.aspect.r9x16" },
  { id: "1:1", width: 1, height: 1, labelKey: "preview.aspect.r1x1" },
  { id: "4:5", width: 4, height: 5, labelKey: "preview.aspect.r4x5" },
  { id: "16:9", width: 16, height: 9, labelKey: "preview.aspect.r16x9" },
] as const satisfies readonly AspectDef[];
export type AspectId = (typeof ASPECTS)[number]["id"];

/** A saved workspace: where the panels sit (ratios, never pixels) and the preview it opens with. */
export interface WorkspaceDef {
  id: WorkspaceId;
  labelKey: TKey;
  defaultAspect: AspectId;
  /** Share of the dock width given to the asset browser and the inspector. */
  assetsWidth: number;
  inspectorWidth: number;
  /** Share of the dock height given to the timeline. */
  timelineHeight: number;
}

export const WORKSPACE_DEFS: Record<WorkspaceId, WorkspaceDef> = {
  "short-form": {
    id: "short-form",
    labelKey: "workspace.short-form",
    defaultAspect: "9:16",
    assetsWidth: 0.24,
    inspectorWidth: 0.22,
    timelineHeight: 0.32,
  },
  "long-form": {
    id: "long-form",
    labelKey: "workspace.long-form",
    defaultAspect: "16:9",
    assetsWidth: 0.2,
    inspectorWidth: 0.22,
    timelineHeight: 0.42,
  },
};

export const TOOL_MODES = ["select", "razor", "ripple"] as const;
export type ToolMode = (typeof TOOL_MODES)[number];

/**
 * The part of the view the engine keeps per project (`view.get` / `view.set`, WP-1C.04):
 * playhead, zoom, scroll and selection; the panel layout rides along as `panel_layout` (see
 * `viewSync.ts`). It is kept out of localStorage: the engine is the source. The project's
 * `preview_quality` is not restored: the preview's quality is the person's preference
 * (`playback.quality_playing`, WP-2H.10).
 */
export interface EngineViewState {
  /** Playhead position, flicks. */
  playhead: number;
  /** Timeline zoom in pixels per second; `null` until set (the timeline picks a default). */
  zoom: number | null;
  /** Timeline scroll offset. */
  scroll: { x: number; y: number };
  /** Selected entities, in selection order. */
  selection: SelectionRef[];
  /**
   * The Source view's in and out marks per media asset (WP-2A.14), flicks in the media's own
   * time. Only assets with a mark are listed. The engine keeps them with the project's view
   * state (`ui-state.json`), so they survive a reopen; they are not part of the document.
   */
  sourceMarks: SourceMarksMap;
}

/** The marks of every marked asset, by media id. */
export type SourceMarksMap = Readonly<Partial<Record<MediaId, SourceMarks>>>;

const NO_MARKS: SourceMarksMap = {};

export const initialEngineView: EngineViewState = {
  playhead: 0,
  zoom: null,
  scroll: { x: 0, y: 0 },
  selection: [],
  sourceMarks: NO_MARKS,
};

export interface ViewState extends EngineViewState {
  /** Session only: the active timeline tool. */
  toolMode: ToolMode;
  /** Session only: timeline snapping to playhead and clip edges (`N`, WP-1M.06). */
  snapping: boolean;
  /**
   * Master output gain (linear, `0..=4`, 1 = unity): the transport bar's volume (WP-1M.04). The
   * engine keeps it per project (`master_gain`); the transport sends it as the slider moves
   * (`view.set`), so `viewSync` only restores it.
   */
  masterGain: number;
  /** Session only: the output is muted (the engine gets `master_gain` 0, WP-1M.04). */
  muted: boolean;
  /** Session only: safe-area guides over the preview (WP-1M.04). */
  safeAreas: boolean;
  /**
   * Session only: the clips whose keyframe lanes are open on the timeline (WP-2C.05), by item
   * ID. View state, not project data: nothing here is saved or sent to the engine.
   */
  keyframeLanes: readonly string[];
  workspaceId: WorkspaceId;
  assetTab: AssetTabId;
  aspect: AspectId;
  /** Last dockview layout per workspace; absent means "use the workspace default". */
  layouts: Partial<Record<WorkspaceId, SerializedDockview>>;
  /** Bumped when a layout must be rebuilt from its default (reset). */
  layoutRevision: number;
}

export interface ViewActions {
  switchWorkspace: (id: WorkspaceId) => void;
  saveLayout: (id: WorkspaceId, layout: SerializedDockview) => void;
  resetWorkspace: (id: WorkspaceId) => void;
  setAssetTab: (tab: AssetTabId) => void;
  setAspect: (aspect: AspectId) => void;
  setToolMode: (mode: ToolMode) => void;
  setSnapping: (on: boolean) => void;
  setPlayhead: (flicks: number) => void;
  setZoom: (pxPerSec: number | null) => void;
  setScroll: (x: number, y: number) => void;
  setSelection: (selection: SelectionRef[]) => void;
  /** Replaces the marks of `media`; `null` (or marks with neither end) clears them. */
  setSourceMarks: (media: MediaId, marks: SourceMarks | null) => void;
  setMasterGain: (gain: number) => void;
  setMuted: (muted: boolean) => void;
  setSafeAreas: (on: boolean) => void;
  /** Replaces the clips whose keyframe lanes are open. */
  setKeyframeLanes: (items: readonly string[]) => void;
  /**
   * Takes the engine's view state (`view.get` on open, `view.changed` from another client)
   * and, when it carries a panel layout, the workspace, aspect and layouts. `viewSync` calls it;
   * nothing it sets is sent back.
   */
  applyEngineView: (ui: Partial<UiState>) => void;
}

export type ViewStore = ViewState & ViewActions;

export const initialViewState: ViewState = {
  ...initialEngineView,
  toolMode: "select",
  snapping: true,
  masterGain: 1,
  muted: false,
  safeAreas: false,
  keyframeLanes: [],
  workspaceId: "short-form",
  assetTab: "media",
  aspect: WORKSPACE_DEFS["short-form"].defaultAspect,
  layouts: {},
  layoutRevision: 0,
};

export const VIEW_STORAGE_KEY = "cutweave.view";

const isOneOf = <T extends string>(list: readonly T[], value: unknown): value is T =>
  typeof value === "string" && (list as readonly string[]).includes(value);

/** Keeps only well-formed persisted values so a stale or hand-edited blob cannot break startup. */
function sanitize(persisted: unknown): Partial<ViewState> {
  if (typeof persisted !== "object" || persisted === null) return {};
  const p = persisted as Record<string, unknown>;
  const out: Partial<ViewState> = {};
  if (isOneOf(WORKSPACES, p.workspaceId)) out.workspaceId = p.workspaceId;
  if (isOneOf(ASSET_TABS, p.assetTab)) out.assetTab = p.assetTab;
  if (typeof p.aspect === "string" && ASPECTS.some((a) => a.id === p.aspect)) {
    out.aspect = p.aspect as AspectId;
  }
  if (typeof p.layouts === "object" && p.layouts !== null) {
    const layouts: Partial<Record<WorkspaceId, SerializedDockview>> = {};
    for (const id of WORKSPACES) {
      const layout = (p.layouts as Record<string, unknown>)[id];
      if (typeof layout === "object" && layout !== null) layouts[id] = layout as SerializedDockview;
    }
    out.layouts = layouts;
  }
  return out;
}

/** The panel layout as stored in the engine's `panel_layout` (opaque to it). */
export interface PanelLayout {
  workspaceId: WorkspaceId;
  aspect: AspectId;
  layouts: Partial<Record<WorkspaceId, SerializedDockview>>;
}

/** Version tag of the `panel_layout` blob; bump when its shape changes. */
export const PANEL_LAYOUT_VERSION = 1;

/** The `panel_layout` value to store in the engine for the current view state. */
export function panelLayoutOf(s: Pick<ViewState, "workspaceId" | "aspect" | "layouts">): unknown {
  return {
    v: PANEL_LAYOUT_VERSION,
    workspaceId: s.workspaceId,
    aspect: s.aspect,
    layouts: s.layouts,
  };
}

/**
 * Narrows the engine's opaque `panel_layout` (`unknown`, `null` until the UI first sets it) to
 * what this build understands; `null` when it is absent, from another version or malformed.
 */
export function panelLayoutFrom(value: unknown): PanelLayout | null {
  if (typeof value !== "object" || value === null) return null;
  const v = value as Record<string, unknown>;
  if (v.v !== PANEL_LAYOUT_VERSION) return null;
  const clean = sanitize({ workspaceId: v.workspaceId, aspect: v.aspect, layouts: v.layouts });
  if (clean.workspaceId === undefined || clean.aspect === undefined) return null;
  return { workspaceId: clean.workspaceId, aspect: clean.aspect, layouts: clean.layouts ?? {} };
}

/** The browser's localStorage when it works; `undefined` (no persistence) when it does not. */
function browserStorage(): StateStorage | undefined {
  try {
    const probe = `${VIEW_STORAGE_KEY}.probe`;
    globalThis.localStorage.setItem(probe, "1");
    globalThis.localStorage.removeItem(probe);
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}

function memoryStorage(): StateStorage {
  const data = new Map<string, string>();
  return {
    getItem: (key) => data.get(key) ?? null,
    setItem: (key, value) => void data.set(key, value),
    removeItem: (key) => void data.delete(key),
  };
}

/**
 * Builds a view store. Pass `storage` to persist (the app uses localStorage); omit it for an
 * in-memory store. The persisted subset is workspace, asset tab, preview aspect
 * and per-workspace layouts; WP-1M.02 later mirrors the layout part to the engine (`view.set`).
 */
export function createViewStore(storage?: StateStorage) {
  const medium = storage ?? memoryStorage();
  return createStore<ViewStore>()(
    persist(
      (set) => ({
        ...initialViewState,
        switchWorkspace: (id) =>
          set((s) =>
            s.workspaceId === id
              ? {}
              : { workspaceId: id, aspect: WORKSPACE_DEFS[id].defaultAspect },
          ),
        saveLayout: (id, layout) => set((s) => ({ layouts: { ...s.layouts, [id]: layout } })),
        resetWorkspace: (id) =>
          set((s) => {
            const { [id]: _dropped, ...layouts } = s.layouts;
            return { layouts, layoutRevision: s.layoutRevision + 1 };
          }),
        setAssetTab: (assetTab) => set(() => ({ assetTab })),
        setAspect: (aspect) => set(() => ({ aspect })),
        setToolMode: (toolMode) => set(() => ({ toolMode })),
        setSnapping: (snapping) => set(() => ({ snapping })),
        setPlayhead: (playhead) => set(() => ({ playhead: Math.max(0, Math.round(playhead)) })),
        setZoom: (zoom) => set(() => ({ zoom })),
        setScroll: (x, y) => set(() => ({ scroll: { x: Math.max(0, x), y: Math.max(0, y) } })),
        setSelection: (selection) => set(() => ({ selection })),
        setSourceMarks: (media, marks) =>
          set((s) => {
            const { [media]: _old, ...rest } = s.sourceMarks;
            const empty = marks === null || (marks.in == null && marks.out == null);
            if (empty) return _old === undefined ? {} : { sourceMarks: rest };
            return {
              sourceMarks: { ...rest, [media]: { in: marks.in ?? null, out: marks.out ?? null } },
            };
          }),
        setMasterGain: (gain) =>
          set(() => (Number.isFinite(gain) ? { masterGain: Math.min(4, Math.max(0, gain)) } : {})),
        setMuted: (muted) => set(() => ({ muted })),
        setSafeAreas: (safeAreas) => set(() => ({ safeAreas })),
        setKeyframeLanes: (keyframeLanes) => set(() => ({ keyframeLanes })),
        applyEngineView: (ui) =>
          set((s) => {
            const next: Partial<ViewState> = {};
            if (typeof ui.playhead === "number") next.playhead = ui.playhead;
            if (ui.zoom !== undefined) next.zoom = ui.zoom;
            if (ui.scroll !== undefined) {
              next.scroll = { x: ui.scroll.x ?? s.scroll.x, y: ui.scroll.y ?? s.scroll.y };
            }
            if (ui.selection !== undefined) next.selection = ui.selection;
            if (typeof ui.master_gain === "number") next.masterGain = ui.master_gain;
            if (typeof ui.source_marks === "object" && ui.source_marks !== null) {
              next.sourceMarks = ui.source_marks;
            }
            const layout = panelLayoutFrom(ui.panel_layout);
            if (layout !== null) {
              Object.assign(next, layout, { layoutRevision: s.layoutRevision + 1 });
            }
            return next;
          }),
      }),
      {
        name: VIEW_STORAGE_KEY,
        version: 1,
        storage: createJSONStorage(() => medium),
        partialize: (s) => ({
          workspaceId: s.workspaceId,
          assetTab: s.assetTab,
          aspect: s.aspect,
          layouts: s.layouts,
        }),
        merge: (persisted, current) => ({ ...current, ...sanitize(persisted) }),
      },
    ),
  );
}

export const viewStore = createViewStore(browserStorage());

export function useViewStore<T>(selector: (state: ViewStore) => T): T {
  return useStore(viewStore, selector);
}
