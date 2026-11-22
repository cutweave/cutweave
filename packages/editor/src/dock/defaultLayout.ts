// SPDX-License-Identifier: GPL-3.0-or-later
import type { DockviewApi } from "dockview-react";
import { i18n, type TKey } from "../i18n/index.ts";
import { WORKSPACE_DEFS, type WorkspaceId } from "../state/viewStore.ts";
import { tokenPx } from "./theme.ts";

export const PANEL_IDS = ["assets", "preview", "inspector", "timeline"] as const;
export type PanelId = (typeof PANEL_IDS)[number];

export const PANEL_TITLE_KEYS: Record<PanelId, TKey> = {
  assets: "panel.assets",
  preview: "panel.preview",
  inspector: "panel.inspector",
  timeline: "panel.timeline",
};

function panelOptions(id: PanelId) {
  return {
    id,
    component: id,
    tabComponent: "header",
    title: i18n.t(PANEL_TITLE_KEYS[id]),
    params: { titleKey: PANEL_TITLE_KEYS[id] },
  };
}

/**
 * Builds the CapCut-familiar arrangement for a workspace: assets left, preview centre, inspector
 * right, timeline across the bottom. Sizes are shares of the dock (see WORKSPACE_DEFS) and
 * minimums come from layout tokens, so no pixel literal lives here.
 */
export function applyDefaultLayout(api: DockviewApi, workspace: WorkspaceId): void {
  const def = WORKSPACE_DEFS[workspace];
  const sideMin = tokenPx("--layer-width-sm");
  const stageMin = tokenPx("--layer-width-md");
  const timelineMin = tokenPx("--track-md") * 3;

  api.clear();
  const assets = api.addPanel({ ...panelOptions("assets"), minimumWidth: sideMin });
  const preview = api.addPanel({
    ...panelOptions("preview"),
    minimumWidth: stageMin,
    position: { referencePanel: assets, direction: "right" },
  });
  const inspector = api.addPanel({
    ...panelOptions("inspector"),
    minimumWidth: sideMin,
    position: { referencePanel: preview, direction: "right" },
  });
  const timeline = api.addPanel({
    ...panelOptions("timeline"),
    minimumHeight: timelineMin,
    position: { direction: "below" },
  });

  const { width, height } = api;
  if (width > 0 && height > 0) {
    assets.api.setSize({ width: Math.round(width * def.assetsWidth) });
    inspector.api.setSize({ width: Math.round(width * def.inspectorWidth) });
    timeline.api.setSize({ height: Math.round(height * def.timelineHeight) });
  }
  preview.api.setActive();
}
