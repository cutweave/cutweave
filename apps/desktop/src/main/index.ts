// SPDX-License-Identifier: GPL-3.0-or-later
// Electron main process entry. Wires: the hardened app:// protocol (CSP per response), session
// and webContents hardening, the window factory, validated IPC, the application menu (native on
// macOS only; the editor draws its own on Windows and Linux, WP-1M.09) and the engine supervisor.
// Security controls: WP-1L.02, see SECURITY-MODEL.md.
// Supervision, restart and the failed dialog: WP-1L.03 (src/main/engine/supervisor.ts).

import { mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import {
  app,
  BrowserWindow,
  clipboard,
  dialog,
  ipcMain,
  Menu,
  nativeTheme,
  net,
  protocol,
  session,
  shell,
} from "electron";
import { type PreferenceKey, resolveTheme } from "../../../../packages/preferences/src/index.ts";
import type { EditAction, EngineStatus, ThemeId } from "../shared/api.ts";
import {
  addFolder,
  clientSetups,
  engineAllowRoots,
  resolveChosenFolder,
  resolveCliPath,
} from "./agents.ts";
import {
  APP_ORIGIN,
  APP_SCHEME,
  registerAppProtocol,
  registerPluginProtocolStub,
} from "./app-protocol.ts";
import { CloseGuard } from "./close-guard.ts";
import { appResponseHeaders, buildCsp } from "./csp.ts";
import { type EngineFailedChoice, showEngineFailedDialog } from "./engine/failed-dialog.ts";
import { devAllowRoots, resolveEngineLaunch } from "./engine/launch.ts";
import { pickFreePort } from "./engine/ports.ts";
import { EngineSupervisor } from "./engine/supervisor.ts";
import { createHandlers, MEDIA_FILE_FILTERS } from "./handlers.ts";
import { type HardeningContext, hardenSession, hardenWebContents } from "./hardening.ts";
import { createSenderCheck, registerIpc } from "./ipc.ts";
import { applicationMenuTemplate } from "./menu.ts";
import { PreferencesStore } from "./preferences-store.ts";
import { forcedPreviewPath } from "./preview-path.ts";
import { RecentProjects } from "./recent-projects.ts";
import { installTestHooks, shouldInstallTestHooks } from "./test-hooks.ts";
import { hasThemedOverlay, titleBarOverlay } from "./title-bar.ts";
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
let mainWindow: BrowserWindow | null = null;
/** Unsaved-changes guard of the editor window (WP-1M.07). */
const closeGuard = new CloseGuard();
/** Recent projects; created once the profile folder is final (`app.whenReady`). */
let recentProjects: RecentProjects | null = null;
/** The preferences (WP-1M.11); created with the recent list, before the first window. */
let preferences: PreferencesStore | null = null;
let openExternal = (url: string): Promise<void> => shell.openExternal(url);
/** Asks Restart / Quit when the engine is `failed`; the e2e hooks answer it instead. */
let askEngineFailed = (status: EngineStatus): Promise<EngineFailedChoice> =>
  showEngineFailedDialog(dialog, mainWindow, status, supervisor?.project ?? null);

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

/** The editor window, while it is open and not being torn down. */
function liveWindow(): BrowserWindow | null {
  return mainWindow !== null && !mainWindow.isDestroyed() ? mainWindow : null;
}

/** Edit > Cut, Copy, Paste, Select all: what the native menu's edit roles call. */
function runEditCommand(action: EditAction): void {
  const contents = liveWindow()?.webContents;
  if (contents === undefined) return;
  switch (action) {
    case "cut":
      contents.cut();
      return;
    case "copy":
      contents.copy();
      return;
    case "paste":
      contents.paste();
      return;
    case "selectAll":
      contents.selectAll();
      return;
  }
}

/** The person's Documents folder; `<home>/Documents` where the OS has none to give. */
function documentsFolder(): string {
  try {
    return app.getPath("documents");
  } catch {
    return path.join(app.getPath("home"), "Documents");
  }
}

/** The theme the editor shows for the preferences: `system` follows the OS. */
function currentTheme(): ThemeId {
  return resolveTheme(
    preferences?.get("appearance.theme") ?? "dark",
    nativeTheme.shouldUseDarkColors,
  );
}

/** Status to every editor window (never the token: that is `engine:getConnection` only). */
function setEngineStatus(status: EngineStatus): void {
  engineStatus = status;
  for (const win of windowsOfKind("editor")) win.webContents.send("engine:status", status);
}

/** Initial port: the OS's choice; unpackaged runs may pin it (tests force a taken port). */
async function chooseInitialPort(): Promise<number> {
  const pinned = Number(process.env.CUTWEAVE_DEV_ENGINE_PORT);
  if (isDev && Number.isInteger(pinned) && pinned > 0 && pinned < 65_536) return pinned;
  return pickFreePort();
}

/**
 * Supervise the engine (WP-1L.03). The port was chosen before the first window loaded, so the
 * CSP already names it; when the supervisor has to move to another port, the CSP follows at
 * once and the editor windows reload as soon as the engine on the new port is ready.
 */
function superviseEngine(
  launch: NonNullable<ReturnType<typeof resolveEngineLaunch>>,
  port: number,
  allowRoots: () => readonly string[],
): EngineSupervisor {
  let reloadOnReady = false;
  const sup = new EngineSupervisor({
    launch,
    port,
    // The editor's origin (and the Vite dev server's in dev) may open the engine's socket.
    extraArgs: [
      "--allow-origin",
      APP_ORIGIN,
      ...(devServerOrigin === undefined ? [] : ["--allow-origin", devServerOrigin]),
    ],
    // Folders agents may use (WP-1J.05): `--allow-root` at every start and restart.
    allowRoots,
    clientVersion: app.getVersion(),
    onPortChange: (next) => {
      enginePort = next;
      reloadOnReady = true;
    },
    log: (message) => console.warn(`[engine] ${message}`),
  });
  sup.onStatus((status) => {
    setEngineStatus(status);
    if (status.state === "ready" && reloadOnReady) {
      reloadOnReady = false;
      for (const win of windowsOfKind("editor")) win.webContents.reload();
    }
    if (status.state === "failed") {
      void askEngineFailed(status).then((choice) => {
        if (choice === "restart") void sup.restart();
        else app.quit();
      });
    }
  });
  return sup;
}

function openWindow(): void {
  mainWindow = createMainWindow({
    stateFile: path.join(app.getPath("userData"), "window-state.json"),
    url: devServerUrl ?? `${APP_ORIGIN}/`,
    factory,
    // Read before the window exists, so its frame and controls open in the right colors.
    theme: currentTheme(),
  });
  closeGuard.reset();
  // With unsaved changes the editor settles them first (Save / Discard / Cancel); it answers
  // with `window:confirmClose`, which closes the window again and lets this through.
  mainWindow.on("close", (event) => {
    const win = mainWindow;
    if (win === null || win.isDestroyed()) return;
    const alive = !win.webContents.isDestroyed() && !win.webContents.isCrashed();
    if (closeGuard.holds(alive)) {
      event.preventDefault();
      win.webContents.send("window:closeRequested", true);
    }
  });
  // The engine records when its window last had the focus, so an agent that does not name an
  // engine gets the one the person is looking at (WP-1J.04).
  mainWindow.on("focus", () => supervisor?.setWindowFocused(true));
  mainWindow.on("blur", () => supervisor?.setWindowFocused(false));
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
    const testHooks = shouldInstallTestHooks(app.isPackaged, process.env);
    const prefs = new PreferencesStore(path.join(app.getPath("userData"), "preferences.json"), {
      // The one default that depends on this machine (arch §16, docs/reference/settings.md).
      contextDefaults: {
        "general.new_project_folder": path.join(documentsFolder(), "Cutweave"),
      },
      log: (message) => console.warn(`[prefs] ${message}`),
    });
    preferences = prefs;
    // `Match system` follows the OS live (the page's `prefers-color-scheme` follows this).
    nativeTheme.themeSource = prefs.get("appearance.theme");
    recentProjects = new RecentProjects(
      path.join(app.getPath("userData"), "recent-projects.json"),
      undefined,
      () => prefs.get("general.recent_count"),
    );
    // The folders agents may use (WP-1J.05): the development ones, then the person's that still
    // exist. Read at every engine start and restart, and sent to the engine when they change.
    const devRoots = devAllowRoots(process.env, app.isPackaged);
    const agentRoots = (): string[] =>
      engineAllowRoots(devRoots, prefs.get("agents.allowed_folders"), {
        platform: process.platform,
        log: (message) => console.warn(`[agents] ${message}`),
      });
    /** The engine's answer to the latest change of the folders. */
    let rootsApplied: Promise<void> = Promise.resolve();
    // The bundled CLI agents run (Settings > Agents and MCP shows its setups).
    const cliPath = resolveCliPath({
      env: process.env,
      isPackaged: app.isPackaged,
      resourcesPath: process.resourcesPath,
      repoRoot: path.resolve(__dirname, "..", "..", "..", ".."),
      platform: process.platform,
    });
    prefs.onChange((changes) => {
      if (changes["appearance.theme"] !== undefined) {
        nativeTheme.themeSource = changes["appearance.theme"];
      }
      if (changes["agents.allowed_folders"] !== undefined) {
        rootsApplied = supervisor?.applyAllowedRoots() ?? Promise.resolve();
      }
      for (const win of windowsOfKind("editor")) win.webContents.send("prefs:changed", changes);
    });
    const checkSender = createSenderCheck({
      devServerOrigin,
      isEditorContents: (sender) => windowKindOf(sender) === "editor",
    });
    // The one synchronous call, from the preload before the page loads: the theme to paint, so
    // the first frame is in the right colors. It answers `dark` or `light` and nothing else.
    ipcMain.on("prefs:initialTheme", (event) => {
      event.returnValue = checkSender(event) === null ? currentTheme() : "dark";
    });
    registerIpc(
      ipcMain,
      createHandlers({
        appInfo: {
          name: app.getName(),
          version: app.getVersion(),
          platform: process.platform,
          ...(isDev ? { dev: true as const } : {}),
          ...(testHooks ? { testHooks: true as const } : {}),
          ...(forcedPreviewPath(process.env) === "jpeg" ? { preview: "jpeg" as const } : {}),
        },
        engineStatus: () => engineStatus,
        engineCredentials: () => supervisor?.credentials ?? null,
        chooseFolder: async () => {
          const result = await dialog.showOpenDialog({
            properties: ["openDirectory", "createDirectory"],
          });
          return result.canceled ? null : (result.filePaths[0] ?? null);
        },
        recentProjects: {
          list: () => recentProjects?.list() ?? [],
          add: (p) => recentProjects?.add(p),
          remove: (p) => recentProjects?.remove(p),
          clear: () => recentProjects?.clear(),
        },
        setWindowDirty: (dirty) => closeGuard.setDirty(dirty),
        confirmWindowClose: () => {
          closeGuard.confirm();
          mainWindow?.close();
        },
        chooseMediaFiles: async () => {
          const result = await dialog.showOpenDialog({
            properties: ["openFile", "multiSelections"],
            filters: MEDIA_FILE_FILTERS,
          });
          return result.canceled ? null : result.filePaths;
        },
        chooseExportFile: async (defaultPath) => {
          const result = await dialog.showSaveDialog({
            defaultPath,
            filters: [{ name: "MP4 video", extensions: ["mp4"] }],
            properties: ["createDirectory"],
          });
          return result.canceled ? null : (result.filePath ?? null);
        },
        showItemInFolder: (p) => shell.showItemInFolder(p),
        openExternal: (url) => openExternal(url),
        about: () => ({
          name: app.getName(),
          version: app.getVersion(),
          electron: process.versions.electron,
          chrome: process.versions.chrome,
          node: process.versions.node,
          v8: process.versions.v8,
          platform: `${process.platform} ${process.arch}`,
        }),
        quit: () => app.quit(),
        toggleFullScreen: () => {
          const win = liveWindow();
          win?.setFullScreen(!win.isFullScreen());
        },
        setTheme: (theme) => {
          if (hasThemedOverlay(process.platform)) {
            liveWindow()?.setTitleBarOverlay(titleBarOverlay(theme));
          }
        },
        editCommand: runEditCommand,
        prefs: {
          snapshot: () => ({ values: prefs.snapshot(), defaults: prefs.defaults() }),
          set: (key, value) => prefs.set(key, value),
          reset: (keys: readonly PreferenceKey[]) => prefs.reset(keys),
          exportText: () => prefs.exportText(),
          importText: (text) => prefs.importText(text),
        },
        chooseSettingsExportFile: async (defaultName) => {
          const result = await dialog.showSaveDialog({
            defaultPath: path.join(documentsFolder(), defaultName),
            filters: [{ name: "Cutweave settings", extensions: ["json"] }],
            properties: ["createDirectory"],
          });
          return result.canceled ? null : (result.filePath ?? null);
        },
        chooseSettingsImportFile: async () => {
          const result = await dialog.showOpenDialog({
            properties: ["openFile"],
            filters: [{ name: "Cutweave settings", extensions: ["json"] }],
          });
          return result.canceled ? null : (result.filePaths[0] ?? null);
        },
        writeTextFile: (file, text) => writeFileSync(file, text, "utf8"),
        readTextFile: (file, maxBytes) => {
          try {
            if (statSync(file).size > maxBytes) return null;
            return readFileSync(file, "utf8");
          } catch {
            return null;
          }
        },
        openAppFolder: async (which) => {
          const folder = app.getPath(which === "logs" ? "logs" : "userData");
          mkdirSync(folder, { recursive: true });
          await shell.openPath(folder);
        },
        agents: {
          chooseFolder: async () => {
            const result = await dialog.showOpenDialog({
              title: "Choose a folder agents can use",
              properties: ["openDirectory"],
            });
            return result.canceled ? null : (result.filePaths[0] ?? null);
          },
          resolveFolder: (chosen) => resolveChosenFolder(chosen),
          folders: () => prefs.get("agents.allowed_folders"),
          add: (folders, folder) => addFolder(folders, folder, process.platform),
          store: async (folders) => {
            prefs.set("agents.allowed_folders", folders);
            await rootsApplied;
          },
          connectInfo: () => ({
            cliPath,
            setups:
              cliPath === null
                ? []
                : clientSetups(cliPath, prefs.get("agents.client_profile"), process.platform),
          }),
          writeClipboard: (text) => clipboard.writeText(text),
        },
        dev: isDev
          ? {
              reload: () => liveWindow()?.webContents.reload(),
              toggleDevTools: () => liveWindow()?.webContents.toggleDevTools(),
            }
          : undefined,
      }),
      checkSender,
    );
    // macOS keeps its native app menu; on Windows and Linux `null` removes the menu bar (the
    // editor's title bar has its own, WP-1M.09).
    const menuTemplate = applicationMenuTemplate({
      platform: process.platform,
      appName: app.getName(),
      isDev,
      send: (command) => mainWindow?.webContents.send("menu:command", command),
    });
    Menu.setApplicationMenu(menuTemplate === null ? null : Menu.buildFromTemplate(menuTemplate));
    if (testHooks) {
      installTestHooks({
        createWindow: (kind, options) => createWindow(kind, options, factory),
        appUrl: `${APP_ORIGIN}/`,
        enginePort: () => enginePort,
        captureOpenExternal: (sink) => {
          openExternal = async (url) => {
            sink.push(url);
          };
        },
        engine: () => supervisor,
        captureEngineDialog: (ask) => {
          askEngineFailed = ask;
        },
        windowDirty: () => closeGuard.dirty,
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
    if (launch !== null && enginePort !== null) {
      supervisor = superviseEngine(launch, enginePort, agentRoots);
      supervisor.setWindowFocused(mainWindow?.isFocused() ?? false);
      await supervisor.start();
    }

    app.on("activate", () => {
      if (BrowserWindow.getAllWindows().length === 0) openWindow();
    });
  });

  app.on("window-all-closed", () => {
    if (process.platform !== "darwin") app.quit();
  });

  let quitting = false;
  app.on("before-quit", (event) => {
    // Quitting with unsaved changes goes through the window's close guard first.
    const win = mainWindow;
    if (win !== null && !win.isDestroyed() && !quitting) {
      const alive = !win.webContents.isDestroyed() && !win.webContents.isCrashed();
      if (closeGuard.holds(alive)) {
        event.preventDefault();
        win.close();
        return;
      }
    }
    if (quitting || supervisor === null) return;
    event.preventDefault();
    quitting = true;
    void supervisor.stop().finally(() => app.quit());
  });
}
