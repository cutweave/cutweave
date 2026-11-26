// SPDX-License-Identifier: GPL-3.0-or-later
// The dock's menus (WP-2H.13; the tab's menu moves here from dockview's own, WP-2H.01): a panel's
// tab, and the empty dock.
// TODO(WP-2H.01 follow-up): "Save layout as…" has no saved-layout feature behind it yet.
import {
  detachedPanels,
  isPanelDetached,
  isPanelMaximized,
  openPanelsStore,
} from "../../workspace/dockHost.ts";
import { registeredPanels } from "../../workspace/panelRegistry.ts";
import type { LabelSpec, MenuContext, MenuRow, MenuSurfaceDef } from "../types.ts";

const panelOf = (ctx: MenuContext) => ({ panel: ctx.ids[0] });

const moveRow = (side: "left" | "right" | "below", label: LabelSpec): MenuRow => ({
  id: `move-${side}`,
  command: "workspace.move_panel",
  group: "panel",
  order: side === "left" ? 0 : side === "right" ? 1 : 2,
  label,
  args: (ctx) => ({ panel: ctx.ids[0], side }),
});

const TAB_ROWS: readonly MenuRow[] = [
  { command: "workspace.close_panel", group: "panel", order: 0, args: panelOf },
  {
    command: "workspace.toggle_maximize",
    group: "panel",
    order: 1,
    args: panelOf,
    label: (ctx) => ({
      key: isPanelMaximized(ctx.ids[0] ?? "")
        ? "contextMenu.item.restore"
        : "contextMenu.item.maximize",
    }),
  },
  { command: "workspace.float_panel", group: "panel", order: 2, args: panelOf },
  // Its own OS window (WP-2H.14, ADR-0031).
  {
    command: "workspace.popout_panel",
    group: "panel",
    order: 3,
    args: panelOf,
    enabledIf: (ctx) => !isPanelDetached(ctx.ids[0] ?? ""),
    reason: "contextMenu.reason.inOwnWindow",
  },
  {
    id: "move",
    group: "panel",
    order: 5,
    submenu: {
      label: { key: "contextMenu.item.moveTo" },
      rows: [
        moveRow("left", { key: "contextMenu.item.left" }),
        moveRow("right", { key: "contextMenu.item.right" }),
        moveRow("below", { key: "contextMenu.item.bottom" }),
      ],
    },
  },
  { command: "workspace.reset_layout", group: "layout", order: 0 },
];

/**
 * Back from its own window. Listed only while some panel is detached: a tab's menu opens in the
 * main window today, and in a detached window once WP-2H.15 lands, where this is the row.
 */
const DOCK_BACK_ROW: MenuRow = {
  command: "workspace.dock_panel",
  group: "panel",
  order: 4,
  args: panelOf,
  enabledIf: (ctx) => isPanelDetached(ctx.ids[0] ?? ""),
  reason: "contextMenu.reason.inMainWindow",
};

export const DOCK_TAB: MenuSurfaceDef = {
  id: "dock.tab",
  name: (target) => ({ key: "contextMenu.name.dockTab", vars: { name: target.name ?? "" } }),
  groups: ["panel", "layout"],
  get rows(): readonly MenuRow[] {
    return detachedPanels().length > 0 ? [...TAB_ROWS, DOCK_BACK_ROW] : TAB_ROWS;
  },
};

/** The panels nobody has open, as one row each (read when the menu opens). */
function reopenRows(): readonly MenuRow[] {
  const closed = new Set(openPanelsStore.getState().closed);
  return registeredPanels()
    .filter((panel) => closed.has(panel.id))
    .map(
      (panel, order): MenuRow => ({
        id: `open-${panel.id}`,
        command: "workspace.open_panel",
        group: "panels",
        order,
        label: { key: panel.title },
        args: () => ({ panel: panel.id }),
      }),
    );
}

export const DOCK_EMPTY: MenuSurfaceDef = {
  id: "dock.empty",
  name: () => ({ key: "contextMenu.name.dockEmpty" }),
  groups: ["panels", "layout"],
  get rows(): readonly MenuRow[] {
    return [
      { command: "workspace.reopen_panels", group: "panels", order: 0 },
      {
        id: "reopen",
        group: "panels",
        order: 1,
        submenu: { label: { key: "contextMenu.item.reopenOne" }, rows: reopenRows() },
      },
      { command: "workspace.reset_layout", group: "layout", order: 0 },
    ];
  },
};
