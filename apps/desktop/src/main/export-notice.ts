// SPDX-License-Identifier: GPL-3.0-or-later
// The OS notification of a finished export (WP-2G.04b, Settings > Notifications): shown when the
// person wants it (`notify.export_done`) and Cutweave is in the background, that is, minimized or
// without the focus on the editor window or one of its detached panels. The editor writes the
// text (it has the translations); main decides and shows it, silent (the export sound is the
// editor's own setting). Clicking it brings the editor window back.

import type { ExportNotice } from "../shared/api.ts";

export interface ExportNoticeDeps {
  /** `notify.export_done`. */
  enabled(): boolean;
  /** The OS can show notifications (`Notification.isSupported()`). */
  supported(): boolean;
  /** The editor window is minimized or hidden, or there is none. */
  hidden(): boolean;
  /** The editor window or one of its detached panels has the focus. */
  focused(): boolean;
  /** Shows the notification; `onClick` runs when the person clicks it. */
  show(notice: ExportNotice, onClick: () => void): void;
  /** Restores and focuses the editor window. */
  bringBack(): void;
}

/** Whether an export that just ended is announced by the OS. */
export function shouldNotify(
  deps: Pick<ExportNoticeDeps, "enabled" | "supported" | "hidden" | "focused">,
): boolean {
  if (!deps.enabled() || !deps.supported()) return false;
  return deps.hidden() || !deps.focused();
}

/** Shows the notification when it should be; whether it was shown. */
export function notifyExportFinished(deps: ExportNoticeDeps, notice: ExportNotice): boolean {
  if (!shouldNotify(deps)) return false;
  deps.show(notice, () => deps.bringBack());
  return true;
}
