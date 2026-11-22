// SPDX-License-Identifier: GPL-3.0-or-later
// Electron main process entry (WP-1L.01). Wires: app:// protocol, the main editor window, the
// native menu, validated IPC and the (placeholder) engine supervisor. Hardening is WP-1L.02,
// supervision/restart WP-1L.03.

import { randomBytes } from "node:crypto";
import path from "node:path";
import { app, BrowserWindow, dialog, ipcMain, Menu, net, protocol, shell } from "electron";
import { registerIpc } from "../ipc.ts";
import type { EngineStatus } from "../shared/api.ts";
import { APP_SCHEME, registerAppProtocol } from "./app-protocol.ts";
import { pickFreePort, resolveEngineLaunch } from "./engine/launch.ts";
import { EngineSupervisor } from "./engine/supervisor.ts";
import { createHandlers } from "./handlers.ts";
import { buildMenuTemplate } from "./menu.ts";
import { createMainWindow } from "./window.ts";

// Must run before `ready`.
protocol.registerSchemesAsPrivileged([
  { scheme: APP_SCHEME, privileges: { standard: true, secure: true, supportFetchAPI: true } },
]);

// Tests and dev runs isolate profile data with CUTWEAVE_USER_DATA.
if (process.env.CUTWEAVE_USER_DATA) app.setPath("userData", process.env.CUTWEAVE_USER_DATA);

const bundleDir = path.join(__dirname, "..");
const devServerUrl = app.isPackaged ? undefined : process.env.CUTWEAVE_DEV_URL;

let supervisor: EngineSupervisor | null = null;
let engineStatus: EngineStatus = { state: "unavailable", message: "no engine binary found" };
let engineCredentials: { port: number; token: string } | null = null;
let mainWindow: BrowserWindow | null = null;

function setEngineStatus(status: EngineStatus): void {
  engineStatus = status;
  mainWindow?.webContents.send("engine:status", status);
}

async function startEngine(): Promise<void> {
  const launch = resolveEngineLaunch({
    env: process.env,
    isPackaged: app.isPackaged,
    resourcesPath: process.resourcesPath,
    repoRoot: path.resolve(__dirname, "..", "..", "..", ".."),
    platform: process.platform,
  });
  if (launch === null) return;

  const port = await pickFreePort();
  // 256-bit per-launch UI credential: spawn handshake pipe and preload bridge only (arch §6).
  const token = randomBytes(32).toString("hex");
  supervisor = new EngineSupervisor({ launch, port, uiToken: token });
  supervisor.onStatus((status) => {
    engineCredentials =
      status.state === "ready" && status.port !== undefined ? { port: status.port, token } : null;
    setEngineStatus(status);
  });
  try {
    await supervisor.start();
  } catch (err) {
    console.error("engine failed to start:", err instanceof Error ? err.message : err);
  }
}

function openWindow(): void {
  mainWindow = createMainWindow({
    bundleDir,
    stateFile: path.join(app.getPath("userData"), "window-state.json"),
    devServerUrl,
  });
  mainWindow.on("closed", () => {
    mainWindow = null;
  });
}

if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on("second-instance", () => {
    if (mainWindow?.isMinimized()) mainWindow.restore();
    mainWindow?.focus();
  });

  void app.whenReady().then(async () => {
    registerAppProtocol(protocol, net, path.join(bundleDir, "renderer"));
    registerIpc(
      ipcMain,
      createHandlers({
        appInfo: { name: app.getName(), version: app.getVersion(), platform: process.platform },
        engineStatus: () => engineStatus,
        engineCredentials: () => engineCredentials,
        chooseFolder: async () => {
          const result = await dialog.showOpenDialog({
            properties: ["openDirectory", "createDirectory"],
          });
          return result.canceled ? null : (result.filePaths[0] ?? null);
        },
        showItemInFolder: (p) => shell.showItemInFolder(p),
      }),
    );
    Menu.setApplicationMenu(
      Menu.buildFromTemplate(
        buildMenuTemplate({
          platform: process.platform,
          appName: app.getName(),
          isDev: !app.isPackaged,
          send: (command) => mainWindow?.webContents.send("menu:command", command),
        }),
      ),
    );

    openWindow();
    await startEngine();

    app.on("activate", () => {
      if (BrowserWindow.getAllWindows().length === 0) openWindow();
    });
  });

  app.on("window-all-closed", () => {
    if (process.platform !== "darwin") app.quit();
  });

  let quitting = false;
  app.on("before-quit", (event) => {
    if (quitting || supervisor === null) return;
    event.preventDefault();
    quitting = true;
    void supervisor.stop().finally(() => app.quit());
  });
}
