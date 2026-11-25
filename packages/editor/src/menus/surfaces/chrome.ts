// SPDX-License-Identifier: GPL-3.0-or-later
// The title bar's menus and the toasts' (WP-2H.13): save status, Export, the workspace controls,
// Undo and Redo, a toast, the engine chip. A sash double-click resets its size and has no menu.
// TODO(WP-2H.01): Save layout as…. TODO(WP-2H.11): Open History…, Show in History.
// TODO: Reveal file after an export (the export store keeps no path once it is done), Copy error
// code on a toast (a notice carries no code), Reconnect now (the supervisor has no such call).
import { engineStore } from "../../engine/engineStore.ts";
import { exportStore } from "../../export/exportStore.ts";
import { exportController } from "../../export/useExport.ts";
import { i18n } from "../../i18n/index.ts";
import { sessionStore } from "../../session/sessionStore.ts";
import { toastStore } from "../../state/toastStore.ts";
import type { MenuContext, MenuSurfaceDef } from "../types.ts";

export const CHROME_SAVE: MenuSurfaceDef = {
  id: "chrome.save",
  name: () => ({ key: "contextMenu.name.save" }),
  groups: ["save"],
  rows: [
    {
      command: "chrome.save_now",
      group: "save",
      order: 0,
      when: "engine.live && !project.readOnly",
      reason: "contextMenu.reason.cannotSave",
    },
    { command: "chrome.autosave_settings", group: "save", order: 1 },
  ],
};

export const CHROME_EXPORT: MenuSurfaceDef = {
  id: "chrome.export",
  name: () => ({ key: "contextMenu.name.export" }),
  groups: ["export"],
  rows: [
    {
      command: "export.open",
      group: "export",
      order: 0,
      enabledIf: () => exportStore.getState().active !== null || exportController().available(),
      reason: "contextMenu.reason.nothingToExport",
    },
    {
      id: "cancel-export",
      command: "job.cancel",
      group: "export",
      order: 1,
      label: { key: "contextMenu.item.cancelExport" },
      enabledIf: () => {
        const state = exportStore.getState();
        return state.active !== null && !state.cancelling;
      },
      reason: "contextMenu.reason.noExportRunning",
      args: () => ({ id: exportStore.getState().active?.jobId }),
    },
  ],
};

export const CHROME_WORKSPACE: MenuSurfaceDef = {
  id: "chrome.workspace",
  name: () => ({ key: "contextMenu.name.workspace" }),
  groups: ["layout"],
  rows: [
    {
      id: "reset-this",
      command: "workspace.reset_layout",
      group: "layout",
      order: 0,
      label: { key: "contextMenu.item.resetThisWorkspace" },
    },
    { command: "workspace.reset_all", group: "layout", order: 1 },
  ],
};

const stacks = () => sessionStore.getState().stacks;

export const CHROME_HISTORY: MenuSurfaceDef = {
  id: "chrome.history",
  name: () => ({ key: "contextMenu.name.history" }),
  groups: ["history"],
  rows: [
    {
      command: "edit.undo",
      group: "history",
      order: 0,
      enabledIf: () => stacks().undoDepth > 0,
      reason: "contextMenu.reason.nothingToUndo",
      args: () => ({ actor: "*" }),
    },
    {
      command: "edit.redo",
      group: "history",
      order: 1,
      enabledIf: () => stacks().redoDepth > 0,
      reason: "contextMenu.reason.nothingToRedo",
      args: () => ({ actor: "*" }),
    },
  ],
};

/** What a toast says about itself: its text, for "Copy message". */
export interface ToastData {
  readonly text: string;
}

export const CHROME_TOAST: MenuSurfaceDef = {
  id: "chrome.toast",
  name: () => ({ key: "contextMenu.name.toast" }),
  groups: ["copy", "dismiss"],
  rows: [
    {
      id: "copy-message",
      command: "host.copy_text",
      group: "copy",
      order: 0,
      label: { key: "contextMenu.item.copyMessage" },
      args: (ctx: MenuContext) => ({ text: (ctx.target.data as ToastData).text }),
    },
    {
      command: "toast.dismiss",
      group: "dismiss",
      order: 0,
      args: (ctx) => ({ id: Number(ctx.ids[0]) }),
    },
    {
      command: "toast.dismiss_all",
      group: "dismiss",
      order: 1,
      enabledIf: () => toastStore.getState().notices.length > 1,
      reason: "contextMenu.reason.onlyOneToast",
    },
  ],
};

/** What "Copy diagnostics" puts on the clipboard: the engine's state in plain words. */
export function engineDiagnostics(): string {
  const { connection, blocked } = engineStore.getState();
  const lines = [
    `${i18n.t("contextMenu.diagnostics.connection")}: ${connection}`,
    ...(blocked === null ? [] : [`${i18n.t("contextMenu.diagnostics.blocked")}: ${blocked.kind}`]),
  ];
  return lines.join("\n");
}

export const CHROME_ENGINE: MenuSurfaceDef = {
  id: "chrome.engine",
  name: () => ({ key: "contextMenu.name.engine" }),
  groups: ["engine"],
  rows: [
    {
      id: "copy-diagnostics",
      command: "host.copy_text",
      group: "engine",
      order: 0,
      label: { key: "contextMenu.item.copyDiagnostics" },
      args: () => ({ text: engineDiagnostics() }),
    },
    { command: "host.open_logs_folder", group: "engine", order: 1 },
  ],
};
