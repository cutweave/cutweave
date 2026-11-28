// SPDX-License-Identifier: GPL-3.0-or-later
// The export queue row's menu (`export.row`, WP-2G.04, plan in research 09 §6): Cancel, Retry,
// Move up, Move down, Open file, Reveal file, Copy error (the error code and the message, ADR-0038),
// Remove from list. The queue's own commands are UI
// commands over the engine's `export.*` queue commands, so the list follows at once; reveal and
// copy are the shared host commands; Open file (WP-2G.04b) asks main to open the exported file.
import type { MenuServices } from "../menus/services.ts";
import type { MenuContext, MenuSurfaceDef, MenuTarget } from "../menus/types.ts";
import type { QueueRowData } from "./ExportQueuePanel.tsx";
import { errorText, moveTarget, queueStore } from "./exportQueue.ts";
import { exportQueue } from "./useExport.ts";

const rowOf = (ctx: { readonly target: MenuTarget }): QueueRowData =>
  ctx.target.data as QueueRowData;

const stateIs =
  (...states: QueueRowData["state"][]) =>
  (ctx: MenuContext): boolean =>
    states.includes(rowOf(ctx).state);

const canMove = (direction: "up" | "down") => (ctx: MenuContext) =>
  moveTarget(queueStore.getState().jobs, rowOf(ctx).id, direction) !== undefined;

export const EXPORT_ROW_SURFACE: MenuSurfaceDef = {
  id: "export.row",
  name: (target) => ({ key: "exportQueue.menu.name", vars: { name: target.name ?? "" } }),
  groups: ["run", "order", "file", "list"],
  rows: [
    {
      command: "export.queue_cancel",
      group: "run",
      order: 0,
      enabledIf: stateIs("queued", "running"),
      reason: "exportQueue.menu.notRunning",
    },
    {
      command: "export.queue_retry",
      group: "run",
      order: 1,
      enabledIf: stateIs("failed", "cancelled"),
      reason: "exportQueue.menu.notFailed",
    },
    {
      command: "export.queue_up",
      group: "order",
      order: 0,
      enabledIf: canMove("up"),
      reason: "exportQueue.menu.cannotMove",
    },
    {
      command: "export.queue_down",
      group: "order",
      order: 1,
      enabledIf: canMove("down"),
      reason: "exportQueue.menu.cannotMove",
    },
    {
      command: "export.queue_open",
      group: "file",
      order: 0,
      enabledIf: stateIs("succeeded"),
      reason: "exportQueue.menu.notDone",
    },
    {
      id: "reveal",
      command: "host.reveal_path",
      group: "file",
      order: 1,
      enabledIf: stateIs("succeeded"),
      reason: "exportQueue.menu.notDone",
      args: (ctx) => ({ path: rowOf(ctx).path }),
    },
    {
      id: "copy-error",
      command: "host.copy_text",
      group: "file",
      order: 2,
      label: { key: "exportQueue.menu.copyError" },
      enabledIf: (ctx) => rowOf(ctx).error !== null,
      reason: "exportQueue.menu.noError",
      args: (ctx) => ({ text: errorText(rowOf(ctx).error, rowOf(ctx).errorCode) }),
    },
    {
      command: "export.queue_remove",
      group: "list",
      order: 0,
      enabledIf: stateIs("succeeded", "failed", "cancelled"),
      reason: "exportQueue.menu.notFinished",
    },
  ],
};

/** Registers the queue row's commands and its surface. */
export function registerQueueMenu(services: MenuServices): void {
  const ui = services.ui;
  ui.register({
    id: "export.queue_cancel",
    label: "exportQueue.cancel",
    icon: "x",
    hidden: true,
    run: (_args, ctx) => exportQueue().cancel(rowOf(ctx).id),
  });
  ui.register({
    id: "export.queue_retry",
    label: "exportQueue.retry",
    icon: "refresh-cw",
    hidden: true,
    run: (_args, ctx) => exportQueue().retry(rowOf(ctx).id),
  });
  ui.register({
    id: "export.queue_up",
    label: "exportQueue.moveUp",
    icon: "arrow-up",
    hidden: true,
    run: (_args, ctx) => exportQueue().move(rowOf(ctx).id, "up"),
  });
  ui.register({
    id: "export.queue_down",
    label: "exportQueue.moveDown",
    icon: "arrow-down",
    hidden: true,
    run: (_args, ctx) => exportQueue().move(rowOf(ctx).id, "down"),
  });
  ui.register({
    id: "export.queue_remove",
    label: "exportQueue.remove",
    icon: "trash-2",
    hidden: true,
    run: (_args, ctx) => exportQueue().remove(rowOf(ctx).id),
  });
  ui.register({
    id: "export.queue_open",
    label: "exportQueue.open",
    icon: "external-link",
    hidden: true,
    run: (_args, ctx) => exportQueue().open(rowOf(ctx).path),
  });
  services.surfaces.register(EXPORT_ROW_SURFACE);
}
