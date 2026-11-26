// SPDX-License-Identifier: GPL-3.0-or-later
import { Button, IconButton, Tooltip } from "@cutweave/ui";
import {
  type DockviewApi,
  DockviewReact,
  type DockviewReadyEvent,
  type IDockviewHeaderActionsProps,
  type IDockviewPanelHeaderProps,
  type IWatermarkPanelProps,
} from "dockview-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TKey } from "../i18n/index.ts";
import { ContextSurface, registerContextSurface } from "../menus/host.tsx";
import { EmptyState } from "../panels/EmptyState.tsx";
import { desktopFromWindow } from "../session/desktop.ts";
import { isKeptOnTop, setKeptOnTop } from "../shell/windows.ts";
import { useViewStore, viewStore } from "../state/viewStore.ts";
import "../workspace/builtinPanels.ts";
import { runWorkspaceCommand } from "../workspace/commands.ts";
import { attachDock, loadWorkspace, saveLayoutNow } from "../workspace/dockHost.ts";
import { PANEL_TAB_COMPONENT } from "../workspace/panelOptions.ts";
import { useRegisteredPanels } from "../workspace/panelRegistry.ts";
import { inPanelWindow, type LocatedApi, useLocationWindow, WindowLayers } from "./panelWindow.tsx";
import { dockTheme } from "./theme.ts";

/** Saves are coalesced: a sash drag fires many layout events. */
export const LAYOUT_SAVE_DELAY_MS = 200;

/** A panel's tab: its title, which is also the drag handle. */
function PanelTab({ api, params }: IDockviewPanelHeaderProps<{ titleKey: TKey }>) {
  const { t } = useTranslation();
  const title = t(params.titleKey);
  // Right-click or the Menu key opens the tab's menu: close, maximize, float, move (WP-2H.13). The
  // whole tab is the surface (dockview's own element around this one), not only the title.
  const ref = useRef<HTMLDivElement>(null);
  const panel = api.id;
  // A panel that changes group (floated, into its own window and back) gets a new tab element
  // around this one: the menu moves to that one, once the dock has finished the move.
  const [tab, setTab] = useState<HTMLElement | null>(null);
  useEffect(() => {
    const find = () => {
      const own = ref.current;
      setTab(own?.closest<HTMLElement>("[role='tab']") ?? own);
    };
    find();
    const moved = () => queueMicrotask(find);
    const subs = [api.onDidGroupChange(moved), api.onDidLocationChange(moved)];
    return () => {
      for (const sub of subs) sub.dispose();
    };
  }, [api]);
  useEffect(() => {
    if (tab === null) return undefined;
    return registerContextSurface(
      tab,
      () => ({ surface: "dock.tab", target: { ids: [panel], name: title } }),
      { name: "dock.tab" },
    );
  }, [tab, panel, title]);
  return (
    <div ref={ref} className="cw-dock-tab heading">
      {title}
    </div>
  );
}

/**
 * The close button at the end of a group's tab strip, for its active panel. It cannot sit inside
 * the tab itself: a tab (`role="tab"`) may not contain a button. Closing goes through the
 * `workspace.close_panel` command, which remembers where the panel was docked so View > Panels
 * can bring it back there.
 */
function CloseActivePanel({ activePanel }: Pick<IDockviewHeaderActionsProps, "activePanel">) {
  const { t } = useTranslation();
  if (activePanel === undefined) return null;
  const title = t((activePanel.params as { titleKey: TKey }).titleKey);
  return (
    <IconButton
      icon="x"
      size="sm"
      className="cw-dock-tab__close"
      label={t("panel.closeNamed", { name: title })}
      onClick={() => runWorkspaceCommand("workspace.close_panel", { panel: activePanel.id })}
    />
  );
}

/**
 * Keep on top, in a detached panel's window (WP-2H.14, ADR-0031): main keeps that OS window above
 * the others. Only in the desktop app (a browser has no such window), only in a detached group.
 * Its tooltip shows in that window (WP-2H.15).
 */
function KeepOnTop({ group }: Pick<IDockviewHeaderActionsProps, "group">) {
  const { t } = useTranslation();
  const [detached, setDetached] = useState(group.api.location.type === "popout");
  const [, rerender] = useState(0);
  useEffect(() => {
    const sub = group.api.onDidLocationChange((e) => setDetached(e.location.type === "popout"));
    return () => sub.dispose();
  }, [group]);
  const bridge = desktopFromWindow()?.window;
  if (!detached || bridge?.setPopoutOnTop === undefined) return null;
  const win = group.api.getWindow();
  const on = isKeptOnTop(win);
  const label = t("windows.keepOnTop");
  return (
    <Tooltip label={label}>
      <IconButton
        icon="pin"
        size="sm"
        className="cw-dock-tab__pin"
        label={label}
        pressed={on}
        onClick={() => {
          void bridge.setPopoutOnTop?.(win.name, !on).then((applied) => {
            if (!applied) return;
            setKeptOnTop(win, !on);
            rerender((n) => n + 1);
          });
        }}
      />
    </Tooltip>
  );
}

/** The actions at the end of a group's tab strip, with their tooltips in the group's window. */
function GroupActions({ activePanel, group }: IDockviewHeaderActionsProps) {
  const win = useLocationWindow(group.api as unknown as LocatedApi);
  return (
    <WindowLayers win={win}>
      <KeepOnTop group={group} />
      <CloseActivePanel activePanel={activePanel} />
    </WindowLayers>
  );
}

/** The dock when every panel is closed. */
function EmptyDock(_props: IWatermarkPanelProps) {
  const { t } = useTranslation();
  return (
    <ContextSurface surface="dock.empty" className="cw-dock-empty">
      <EmptyState icon="layout-grid" title={t("dock.empty.title")} body={t("dock.empty.body")}>
        <div className="cw-dock-empty__actions">
          <Button variant="primary" onClick={() => runWorkspaceCommand("workspace.reopen_panels")}>
            {t("dock.empty.reopen")}
          </Button>
          <Button onClick={() => runWorkspaceCommand("workspace.reset_layout")}>
            {t("workspace.reset")}
          </Button>
        </div>
      </EmptyState>
    </ContextSurface>
  );
}

const tabComponents = {
  [PANEL_TAB_COMPONENT]: PanelTab as (p: IDockviewPanelHeaderProps) => React.JSX.Element,
};

/**
 * The dockable panel area. The active workspace's saved layout is restored when it is selected
 * (or its default is built); layout changes are written back to `viewStore` per workspace. The
 * panels come from the registry (`workspace/panelRegistry.ts`); which are open is the dock
 * host's business (`workspace/dockHost.ts`).
 */
export function Dock() {
  const workspaceId = useViewStore((s) => s.workspaceId);
  const revision = useViewStore((s) => s.layoutRevision);
  const registered = useRegisteredPanels();
  const [api, setApi] = useState<DockviewApi | null>(null);
  const activeWorkspace = useRef<string | null>(null);
  const pendingSave = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Each panel knows its window: its menus and tooltips open there once it is detached.
  const components = useMemo(
    () => Object.fromEntries(registered.map((p) => [p.id, inPanelWindow(p.component)])),
    [registered],
  );

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
        saveLayoutNow();
      }, LAYOUT_SAVE_DELAY_MS);
    });
    return () => {
      subscription.dispose();
      cancelSave();
    };
  }, [api, cancelSave]);

  useEffect(() => {
    if (!api) return;
    attachDock(api);
    return () => attachDock(null);
  }, [api]);

  // Load the active workspace; `revision` forces a rebuild after a reset.
  useEffect(() => {
    if (!api) return;
    const previous = activeWorkspace.current;
    if (previous && previous !== workspaceId && pendingSave.current !== null) {
      // Switching: keep the edits made in the workspace we are leaving.
      cancelSave();
      saveLayoutNow();
    } else {
      cancelSave();
    }
    activeWorkspace.current = workspaceId;
    loadWorkspace(api, workspaceId, viewStore.getState().layouts[workspaceId]);
  }, [api, workspaceId, revision, cancelSave]);

  return (
    <DockviewReact
      className="cw-dock"
      components={components}
      tabComponents={tabComponents}
      rightHeaderActionsComponent={GroupActions}
      watermarkComponent={EmptyDock}
      theme={dockTheme()}
      singleTabMode="fullwidth"
      onReady={onReady}
    />
  );
}
