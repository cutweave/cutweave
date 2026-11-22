// SPDX-License-Identifier: GPL-3.0-or-later
import {
  type DockviewApi,
  DockviewReact,
  type DockviewReadyEvent,
  type IDockviewPanelHeaderProps,
} from "dockview-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TKey } from "../i18n/index.ts";
import { AssetBrowserPanel } from "../panels/AssetBrowserPanel.tsx";
import { InspectorPanel } from "../panels/InspectorPanel.tsx";
import { PreviewPanel } from "../panels/PreviewPanel.tsx";
import { TimelinePanel } from "../panels/TimelinePanel.tsx";
import { useViewStore, viewStore, type WorkspaceId } from "../state/viewStore.ts";
import { applyDefaultLayout } from "./defaultLayout.ts";
import { dockTheme } from "./theme.ts";

/** Saves are coalesced: a sash drag fires many layout events. */
export const LAYOUT_SAVE_DELAY_MS = 200;

const components = {
  assets: AssetBrowserPanel,
  preview: PreviewPanel,
  inspector: InspectorPanel,
  timeline: TimelinePanel,
};

/** The group header: a panel title that doubles as the drag handle. No close button. */
function PanelTab({ params }: IDockviewPanelHeaderProps<{ titleKey: TKey }>) {
  const { t } = useTranslation();
  return <div className="cw-dock-tab heading">{t(params.titleKey)}</div>;
}

const tabComponents = { header: PanelTab as (p: IDockviewPanelHeaderProps) => React.JSX.Element };

/**
 * The dockable panel area. The active workspace's saved layout is restored when it is selected
 * (or its default is built); layout changes are written back to `viewStore` per workspace.
 */
export function Dock() {
  const workspaceId = useViewStore((s) => s.workspaceId);
  const revision = useViewStore((s) => s.layoutRevision);
  const [api, setApi] = useState<DockviewApi | null>(null);
  const activeWorkspace = useRef<WorkspaceId | null>(null);
  const pendingSave = useRef<ReturnType<typeof setTimeout> | null>(null);

  const onReady = useCallback((event: DockviewReadyEvent) => setApi(event.api), []);

  const cancelSave = useCallback(() => {
    if (pendingSave.current !== null) clearTimeout(pendingSave.current);
    pendingSave.current = null;
  }, []);

  // Persist layout edits under the workspace that was active when they were made.
  useEffect(() => {
    if (!api) return;
    const subscription = api.onDidLayoutChange(() => {
      cancelSave();
      pendingSave.current = setTimeout(() => {
        pendingSave.current = null;
        const id = activeWorkspace.current;
        if (id) viewStore.getState().saveLayout(id, api.toJSON());
      }, LAYOUT_SAVE_DELAY_MS);
    });
    return () => {
      subscription.dispose();
      cancelSave();
    };
  }, [api, cancelSave]);

  // Load the active workspace; `revision` forces a rebuild after a reset.
  useEffect(() => {
    if (!api) return;
    const previous = activeWorkspace.current;
    if (previous && previous !== workspaceId && pendingSave.current !== null) {
      // Switching: keep the edits made in the workspace we are leaving.
      cancelSave();
      viewStore.getState().saveLayout(previous, api.toJSON());
    } else {
      cancelSave();
    }
    activeWorkspace.current = workspaceId;
    const saved = viewStore.getState().layouts[workspaceId];
    if (saved) {
      try {
        api.fromJSON(saved);
        return;
      } catch {
        // A layout from an older build can no longer be restored; fall back to the default.
      }
    }
    applyDefaultLayout(api, workspaceId);
  }, [api, workspaceId, revision, cancelSave]);

  return (
    <DockviewReact
      className="cw-dock"
      components={components}
      tabComponents={tabComponents}
      theme={dockTheme()}
      singleTabMode="fullwidth"
      disableFloatingGroups
      getTabContextMenuItems={() => []}
      onReady={onReady}
    />
  );
}
