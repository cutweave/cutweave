// SPDX-License-Identifier: GPL-3.0-or-later
// Transport (b) preload (WP-0C.06): runs SANDBOXED (sandbox:true, contextIsolation:true), so it
// can only `require` a handful of Electron modules: no Node, no native addon.
//
// Its only job is the MessagePort handoff: when the page posts WANT_PORT, ask main for a fresh
// channel to the frame-pump utility process and forward the port to the page with
// `window.postMessage(..., [port])`. Nothing is exposed with contextBridge.
//
// It also sends main a one-shot, secret-free self-report (process.sandboxed, whether Node and
// native modules are reachable) for the WP-0C.06 self-test.

import { ipcRenderer } from "electron";
import { PORT_DELIVERY, WANT_PORT } from "./protocol.ts";

ipcRenderer.on("s1b:port", (e) => {
  const [port] = e.ports;
  if (port) window.postMessage(PORT_DELIVERY, window.location.origin, [port]);
});

window.addEventListener("message", (e) => {
  if (e.source === window && e.origin === window.location.origin && e.data === WANT_PORT) {
    ipcRenderer.send("s1b:request-port");
  }
});

function blocked(fn: () => unknown): string {
  try {
    fn();
    return "loaded";
  } catch (e) {
    return `blocked: ${(e as Error).message}`;
  }
}

// `require` is the sandboxed preload's polyfill; a variable path keeps esbuild from bundling.
const req = require as (id: string) => unknown;
const addonPath = ["..", "..", "bridge", "s1_bridge.node"].join("/");
const electron = req("electron") as Record<string, unknown>;
ipcRenderer.send("s1b:preload-report", {
  sandboxed: process.sandboxed === true,
  contextIsolated: process.contextIsolated === true,
  requireFs: blocked(() => req("fs")),
  requireChildProcess: blocked(() => req("child_process")),
  requireNativeAddon: blocked(() => req(addonPath)),
  processDlopen: typeof (process as unknown as { dlopen?: unknown }).dlopen,
  electronModules: Object.keys(electron).sort(),
  // Electron 45: sharedTexture's receiver works in a sandboxed preload (GPU zero-copy path).
  sharedTextureReceiver:
    typeof (electron.sharedTexture as { setSharedTextureReceiver?: unknown } | undefined)
      ?.setSharedTextureReceiver,
});
