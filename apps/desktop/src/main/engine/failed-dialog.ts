// SPDX-License-Identifier: GPL-3.0-or-later
// The dialog main shows when the engine is `failed` (WP-1L.03): what happened and the two ways
// on, Restart (a fresh crash budget; the project is reopened and recovered) or Quit.

import path from "node:path";
import type { BrowserWindow, Dialog, MessageBoxOptions } from "electron";
import type { EngineStatus } from "../../shared/api.ts";

export type EngineFailedChoice = "restart" | "quit";

/**
 * The message box for a failed engine (pure, unit-tested). `project` is the project the engine
 * held, which a restart reopens and recovers.
 */
export function engineFailedDialogOptions(
  status: EngineStatus,
  project: string | null,
): MessageBoxOptions {
  const error = status.lastError;
  const locked = error?.code === "project_locked";
  const next =
    locked && error?.holder
      ? `Close the project in the other Cutweave window (process ${error.holder.pid} on ${error.holder.host}), then restart.`
      : project !== null
        ? `Your edits to ${path.basename(project)} are kept in its journal and come back when the engine restarts.`
        : "Restart the engine to keep editing.";
  const detail = [status.message ?? error?.message ?? "The engine stopped.", next].join("\n\n");
  return {
    type: "error",
    title: "Cutweave",
    message: locked ? "The project is open in another Cutweave window" : "The engine stopped",
    detail,
    buttons: ["Restart", "Quit"],
    defaultId: 0,
    cancelId: 1,
    noLink: true,
  };
}

/** Show the dialog (modal to `parent` when it is open) and return the choice. */
export async function showEngineFailedDialog(
  dialog: Pick<Dialog, "showMessageBox">,
  parent: BrowserWindow | null,
  status: EngineStatus,
  project: string | null,
): Promise<EngineFailedChoice> {
  const options = engineFailedDialogOptions(status, project);
  const result =
    parent !== null && !parent.isDestroyed()
      ? await dialog.showMessageBox(parent, options)
      : await dialog.showMessageBox(options);
  return result.response === 0 ? "restart" : "quit";
}
