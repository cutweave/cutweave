// SPDX-License-Identifier: GPL-3.0-or-later
import type { DockviewApi, IDockviewPanel } from "dockview-react";
import { WORKSPACE_DEFS, type WorkspaceId } from "../state/viewStore.ts";
import { panelOptions } from "../workspace/panelOptions.ts";
import { type PanelDef, registeredPanels } from "../workspace/panelRegistry.ts";

/**
 * Builds a workspace's default arrangement from the panel registry: each panel docks where its
 * `defaultDock` says, once the panel it is docked beside exists (the timeline across the bottom,
 * the preview above it, assets left and the inspector right of the preview, which is the
 * CapCut-familiar layout). Sizes are shares of the dock (see WORKSPACE_DEFS) and minimums come
 * from the registry's layout tokens, so no pixel literal lives here.
 */
export function applyDefaultLayout(api: DockviewApi, workspace: WorkspaceId): void {
  const def = WORKSPACE_DEFS[workspace];
  api.clear();

  const added = new Map<string, IDockviewPanel>();
  let pending: readonly PanelDef[] = registeredPanels();
  while (pending.length > 0) {
    const ready = pending.filter((p) => p.defaultDock.ref === null || added.has(p.defaultDock.ref));
    // A panel docked beside one that does not exist goes to the dock's edge.
    const batch = ready.length > 0 ? ready : [pending[0] as PanelDef];
    for (const panel of batch) {
      const { ref, direction } = panel.defaultDock;
      const reference = ref === null ? undefined : added.get(ref);
      const options = panelOptions(panel);
      const placed =
        added.size === 0
          ? api.addPanel(options)
          : reference !== undefined
            ? api.addPanel({ ...options, position: { referencePanel: reference, direction } })
            : api.addPanel({
                ...options,
                position: { direction: direction === "within" ? "below" : direction },
              });
      added.set(panel.id, placed);
    }
    pending = pending.filter((p) => !added.has(p.id));
  }

  const { width, height } = api;
  if (width > 0 && height > 0) {
    added.get("assets")?.api.setSize({ width: Math.round(width * def.assetsWidth) });
    added.get("inspector")?.api.setSize({ width: Math.round(width * def.inspectorWidth) });
    added.get("timeline")?.api.setSize({ height: Math.round(height * def.timelineHeight) });
  }
  added.get("preview")?.api.setActive();
}
