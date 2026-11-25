// SPDX-License-Identifier: GPL-3.0-or-later
// The History panel's commands and menus (WP-2H.11, plan in research 09): a row's menu
// (`history.row`: Go to this step, Copy label); `history.open` is also a row of the Undo and
// Redo buttons' menu (`chrome.history`, menus/surfaces/chrome.ts). They go in the same tables as every other
// command, so the palette and the keymap (WP-2H.05) list them too.
import type { MenuServices } from "../menus/services.ts";
import type { MenuSurfaceDef, MenuTarget } from "../menus/types.ts";
import { openPanel } from "../workspace/dockHost.ts";
import { goToStep } from "./historyFeed.ts";
import { buildRows } from "./historyModel.ts";
import { historyStore } from "./historyStore.ts";

/** What a history row's menu acts on (`MenuTarget.data`). */
export interface HistoryRowData {
  readonly txId: string;
  /** The step in words ("Split clip"). */
  readonly label: string;
}

const rowData = (ctx: { readonly target: MenuTarget }): HistoryRowData =>
  ctx.target.data as HistoryRowData;

export const HISTORY_ROW_SURFACE: MenuSurfaceDef = {
  id: "history.row",
  name: (target) => ({ key: "history.menu.rowName", vars: { name: target.name ?? "" } }),
  groups: ["go", "copy"],
  rows: [
    { command: "history.go_to", group: "go", order: 0 },
    { command: "history.copy_label", group: "copy", order: 0 },
  ],
};

/** Registers the panel's commands and its row surface. */
export function registerHistoryCommands(services: MenuServices): void {
  const ui = services.ui;
  ui.register({
    id: "history.go_to",
    label: "history.menu.goTo",
    icon: "history",
    hidden: true,
    run: async (_args, ctx) => {
      const { txId } = rowData(ctx);
      await goToStep(buildRows(historyStore.getState().entries), txId);
    },
  });
  ui.register({
    id: "history.copy_label",
    label: "history.menu.copyLabel",
    icon: "copy",
    hidden: true,
    host: true,
    run: async (_args, ctx) => {
      await navigator.clipboard.writeText(rowData(ctx).label);
    },
  });
  ui.register({
    id: "history.open",
    label: "history.menu.open",
    icon: "history",
    hidden: true,
    run: () => openPanel("history"),
  });
  services.surfaces.register(HISTORY_ROW_SURFACE);
}
