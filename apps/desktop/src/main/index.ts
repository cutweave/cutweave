// SPDX-License-Identifier: GPL-3.0-or-later
// Electron main process entry. Wires: the hardened app:// protocol (CSP per response), session
// and webContents hardening, the window factory, validated IPC, the native menu and the
// (placeholder) engine supervisor. Security controls: WP-1L.02, see SECURITY-MODEL.md.
// Supervision/restart: WP-1L.03.

import { randomBytes } from "node:crypto";
import path from "node:path";
import { app, BrowserWindow, dialog, ipcMain, Menu, net, protocol, session, shell } from "electron";
import type { EngineStatus } from "../shared/api.ts";
import {
  APP_ORIGIN,
  APP_SCHEME,
  registerAppProtocol,
  registerPluginProtocolStub,
} from "./app-protocol.ts";
import { appResponseHeaders, buildCsp } from "./csp.ts";
import { pickFreePort, resolveEngineLaunch, startWithPortRetry } from "./engine/launch.ts";
import { EngineSupervisor } from "./engine/supervisor.ts";
import { createHandlers } from "./handlers.ts";
import { type HardeningContext, hardenSession, hardenWebContents } from "./hardening.ts";
import { createSenderCheck, registerIpc } from "./ipc.ts";
import { buildMenuTemplate } from "./menu.ts";
import { installTestHooks, shouldInstallTestHooks } from "./test-hooks.ts";
import { createMainWindow } from "./window.ts";
import { createWindow, type WindowFactoryContext, windowKindOf, windowsOfKind } from "./windows.ts";

// Must run before `ready`. Deliberately no `bypassCSP` and no `allowServiceWorkers`.
protocol.registerSchemesAsPrivileged([
  { scheme: APP_SCHEME, privileges: { standard: true, secure: true, supportFetchAPI: true } },
]);

// Tests and dev runs isolate profile data with CUTWEAVE_USER_DATA.
if (process.env.CUTWEAVE_USER_DATA) app.setPath("userData", process.env.CUTWEAVE_USER_DATA);

const isDev = !app.isPackaged;
const bundleDir = path.join(__dirname, "..");
const devServerUrl = isDev ? process.env.CUTWEAVE_DEV_URL : undefined;
const devServerOrigin = devServerUrl === undefined ? undefined : new URL(devServerUrl).origin;

/** Engine port chosen by main at launch; the CSP is built from it on every app:// response. */
let enginePort: number | null = null;
let supervisor: EngineSupervisor | null = null;
let engineStatus: EngineStatus = { state: "unavailable", message: "no engine binary found" };
let engineCredentials: { port: number; token: string } | null = null;
let mainWindow: BrowserWindow | null = null;
let openExternal = (url: string): Promise<void> => shell.openExternal(url);

const hardening: HardeningContext = { isDev, devServerOrigin, enginePort: () => enginePort };
const factory: WindowFactoryContext = {
  isDev,
  devServerOrigin,
  preloadPath: path.join(bundleDir, "preload", "index.cjs"),
};

// Every webContents (windows, devtools, anything created later) gets the guards, and every
// session the permission/request policy, before any content loads.
app.on("web-contents-created", (_event, contents) => hardenWebContents(contents, hardening));
app.on("session-created", (ses) => hardenSession(ses, hardening));

function setEngineStatus(status: EngineStatus): void {
  engineStatus = status;
  mainWindow?.webContents.send("engine:status", status);
}

/** Initial port: the OS's choice; unpackaged runs may pin it (tests force a taken port). */
async function chooseInitialPort(): Promise<number> {
  const pinned = Number(process.env.CUTWEAVE_DEV_ENGINE_PORT);
  if (isDev && Number.isInteger(pinned) && pinned > 0 && pinned < 65_536) return pinned;
  return pickFreePort();
}

async function startEngine(launch: NonNullable<ReturnType<typeof resolveEngineLaunch>>) {
  // 256-bit per-launch UI credential: spawn handshake pipe and preload bridge only (arch §6).
  const token = randomBytes(32).toString("hex");
  let portChanged = false;
  try {
    await startWithPortRetry({
      initialPort: enginePort ?? (await pickFreePort()),
      pickPort: () => pickFreePort(),
      onPortChange: (port) => {
        enginePort = port;
        portChanged = true;
      },
      start: (port) => {
        supervisor = new EngineSupervisor({ launch, port, uiToken: token });
        supervisor.onStatus((status) => {
          engineCredentials =
            status.state === "ready" && status.port !== undefined
              ? { port: status.port, token }
              : null;
          setEngineStatus(status);
        });
        return supervisor.start();
      },
    });
  } catch (err) {
    console.error("engine failed to start:", err instanceof Error ? err.message : err);
  } finally {
    // The renderer's CSP names the old port: reload so it picks up the new one.
    if (portChanged) for (const win of windowsOfKind("editor")) win.webContents.reload();
  }
}

function openWindow(): void {
  mainWindow = createMainWindow({
    stateFile: path.join(app.getPath("userData"), "window-state.json"),
    url: devServerUrl ?? `${APP_ORIGIN}/`,
    factory,
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
    hardenSession(session.defaultSession, hardening);
    registerAppProtocol(protocol, net, path.join(bundleDir, "renderer"), () =>
      appResponseHeaders(buildCsp(enginePort)),
    );
    registerPluginProtocolStub(protocol);
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
        openExternal: (url) => openExternal(url),
      }),
      createSenderCheck({
        devServerOrigin,
        isEditorContents: (sender) => windowKindOf(sender) === "editor",
      }),
    );
    Menu.setApplicationMenu(
      Menu.buildFromTemplate(
        buildMenuTemplate({
          platform: process.platform,
          appName: app.getName(),
          isDev,
          send: (command) => mainWindow?.webContents.send("menu:command", command),
        }),
      ),
    );
    if (shouldInstallTestHooks(app.isPackaged, process.env)) {
      installTestHooks({
        createWindow: (kind, options) => createWindow(kind, options, factory),
        appUrl: `${APP_ORIGIN}/`,
        enginePort: () => enginePort,
        captureOpenExternal: (sink) => {
          openExternal = async (url) => {
            sink.push(url);
          };
        },
      });
    }

    // The port is chosen before the first load so the renderer's CSP already names it.
    const launch = resolveEngineLaunch({
      env: process.env,
      isPackaged: app.isPackaged,
      resourcesPath: process.resourcesPath,
      repoRoot: path.resolve(__dirname, "..", "..", "..", ".."),
      platform: process.platform,
    });
    if (launch !== null) enginePort = await chooseInitialPort();

    openWindow();
    if (launch !== null) await startEngine(launch);

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
