// SPDX-License-Identifier: GPL-3.0-or-later
// Preload of the MAIN EDITOR WINDOW ONLY (ADR-001 transport (a)): runs with `sandbox: false` so it
// can load the napi addon, with contextIsolation on and nodeIntegration off. The page's main
// world only ever sees `window.cutweave` (src/shared/api.ts), never `require`, `process`,
// `ipcRenderer` or the addon.

import { contextBridge, ipcRenderer } from "electron";
import { createCutweaveApi } from "./api.ts";
import { loadBridgeAddon } from "./bridge-addon.ts";

contextBridge.exposeInMainWorld("cutweave", createCutweaveApi(ipcRenderer, loadBridgeAddon()));
