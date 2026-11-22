// SPDX-License-Identifier: GPL-3.0-or-later
// TEST ONLY (e2e/security.spec.ts): models a compromised renderer that can send ANY IPC message,
// with any channel and arguments. Loaded into a raw test window (never through the factory) to
// prove main rejects unregistered channels and senders that are not editor windows.
const { contextBridge, ipcRenderer } = require("electron");

contextBridge.exposeInMainWorld("forge", {
  invoke: (channel, ...args) => ipcRenderer.invoke(channel, ...args),
});
