// SPDX-License-Identifier: GPL-3.0-or-later
import type { CutweaveApi } from "./shared/api.ts";

declare global {
  interface Window {
    /** Exposed by the main window's preload through `contextBridge` (WP-1L.01). */
    readonly cutweave: CutweaveApi;
  }
}
