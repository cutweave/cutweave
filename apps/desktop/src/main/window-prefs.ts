// SPDX-License-Identifier: GPL-3.0-or-later
// webPreferences for the main editor window. WP-1L.02 turns this into the audited
// `createWindow(kind)` factory (and adds the sandboxed kinds); keep the invariants below.

import type { WebPreferences } from "electron";

/**
 * Main editor window (ADR-001 transport (a)): the preload runs non-sandboxed so it can load the
 * napi addon; everything else stays locked down. Local `app://` content only.
 */
export function mainWindowPreferences(preloadPath: string): WebPreferences {
  return {
    preload: preloadPath,
    contextIsolation: true,
    nodeIntegration: false,
    nodeIntegrationInSubFrames: false,
    sandbox: false,
    webSecurity: true,
    allowRunningInsecureContent: false,
    webviewTag: false,
  };
}
