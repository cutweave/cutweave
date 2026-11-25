// SPDX-License-Identifier: GPL-3.0-or-later
// Builds the `window.cutweave` object from an `ipcRenderer` and the (optional) bridge addon.
// Kept free of `electron` imports so it is unit-testable; `index.ts` wires the real ones.

import type { PreferenceChanges } from "../../../../packages/preferences/src/index.ts";
import type { AttachCanvasResult, CutweaveApi, EngineStatus, MenuCommand } from "../shared/api.ts";
import { type InvokeChannel, type InvokeChannels, isMenuCommand } from "../shared/ipc-contract.ts";
import type { BridgeAddon } from "./bridge-addon.ts";
import {
  browserLoopClock,
  installedPresenter,
  type LoopClock,
  PresentLoop,
  setFramePresenter,
} from "./present-loop.ts";
import { type CanvasPresenter, createWebGpuPresenter, PresenterError } from "./webgpu-presenter.ts";

/** Slice of `ipcRenderer` the preload uses. */
export interface IpcRendererLike {
  invoke(channel: string, ...args: unknown[]): Promise<unknown>;
  on(channel: string, listener: (event: unknown, ...args: unknown[]) => void): unknown;
  removeListener(channel: string, listener: (event: unknown, ...args: unknown[]) => void): unknown;
}

/** Longest path the preload hands back (same bound as the IPC and the editor). */
const MAX_PATH_LENGTH = 4096;

export const ERR_BRIDGE_UNAVAILABLE = "ERR_BRIDGE_UNAVAILABLE";

export function createCutweaveApi(
  ipc: IpcRendererLike,
  addon: BridgeAddon | null,
  /** Present-loop timing: the preload passes the browser's, tests a fake. */
  loopClock: () => LoopClock = browserLoopClock,
  /** `webUtils.getPathForFile` (the preload passes Electron's; it throws for a non-`File`). */
  getPathForFile: (file: File) => string = () => "",
  /** Builds the WebGPU presenter on the page's canvas (WP-1M.04); tests pass a fake. */
  createPresenter: (canvas: HTMLCanvasElement) => Promise<CanvasPresenter> = createWebGpuPresenter,
): CutweaveApi {
  // Each call names its channel; the preload never forwards a caller-chosen channel.
  const invoke = <K extends InvokeChannel>(
    channel: K,
    ...args: InvokeChannels[K]["args"]
  ): Promise<InvokeChannels[K]["result"]> =>
    ipc.invoke(channel, ...args) as Promise<InvokeChannels[K]["result"]>;

  const subscribe = <T>(channel: string, accept: (value: unknown) => T | undefined) => {
    return (listener: (value: T) => void) => {
      const wrapped = (_event: unknown, value: unknown) => {
        const checked = accept(value);
        if (checked !== undefined) listener(checked);
      };
      ipc.on(channel, wrapped);
      return () => {
        ipc.removeListener(channel, wrapped);
      };
    };
  };

  const onCloseRequested = subscribe<true>("window:closeRequested", (v) =>
    v === true ? v : undefined,
  );

  const requireAddon = (): BridgeAddon => {
    if (addon === null) {
      throw new Error(`${ERR_BRIDGE_UNAVAILABLE}: the cutweave-bridge addon is not loaded`);
    }
    return addon;
  };

  // The present loop runs here, in the preload's isolated world (Spike S1 finding 2); only its
  // small stats cross the context bridge.
  let presenting: PresentLoop | null = null;
  const stopPresenting = (): void => {
    presenting?.stop();
  };

  // The WebGPU presenter drawing into the page's canvas (WP-1M.04). The present loop always
  // presents through `installedPresenter`, so attaching or detaching takes effect at once.
  let canvasPresenter: CanvasPresenter | null = null;
  /** Bumped by every attach and detach: an attach that finishes after a newer one is dropped. */
  let attachEpoch = 0;
  const detachCanvas = (): void => {
    attachEpoch += 1;
    setFramePresenter(null);
    canvasPresenter?.destroy();
    canvasPresenter = null;
  };
  const attachCanvas = async (canvas: HTMLCanvasElement): Promise<AttachCanvasResult> => {
    if (addon === null) {
      return { ok: false, reason: "no-addon", message: "the cutweave-bridge addon is not loaded" };
    }
    const isCanvas =
      typeof canvas === "object" &&
      canvas !== null &&
      typeof (canvas as { getContext?: unknown }).getContext === "function";
    if (!isCanvas) return { ok: false, reason: "bad-canvas", message: "not a canvas element" };
    detachCanvas();
    const epoch = attachEpoch;
    try {
      const presenter = await createPresenter(canvas);
      if (epoch !== attachEpoch) {
        presenter.destroy();
        return { ok: false, reason: "device-failed", message: "superseded by a newer attach" };
      }
      canvasPresenter = presenter;
      setFramePresenter(presenter);
      return { ok: true, adapter: presenter.adapter, upload: "writeTexture" };
    } catch (err) {
      const reason = err instanceof PresenterError ? err.reason : "device-failed";
      return { ok: false, reason, message: err instanceof Error ? err.message : String(err) };
    }
  };

  return {
    app: {
      getInfo: () => invoke("app:getInfo"),
      about: () => invoke("app:about"),
      quit: () => invoke("app:quit"),
    },
    engine: {
      getConnection: () => invoke("engine:getConnection"),
      getStatus: () => invoke("engine:getStatus"),
      onStatus: subscribe<EngineStatus>("engine:status", (v) =>
        typeof v === "object" && v !== null && "state" in v ? (v as EngineStatus) : undefined,
      ),
    },
    preview: {
      isAvailable: () => addon !== null,
      open: (ringName, epoch) => {
        const bridge = requireAddon();
        stopPresenting();
        presenting = null;
        return bridge.open(ringName, epoch);
      },
      getLatestFrame: (maxPtsFlicks) => requireAddon().getLatestFrame(maxPtsFlicks),
      waitForFrame: (timeoutMs) => requireAddon().waitForFrame(timeoutMs),
      getClock: () => requireAddon().getClock(),
      getStatus: () => requireAddon().getStatus(),
      startPresenting: (options) => {
        const bridge = requireAddon();
        stopPresenting();
        presenting = new PresentLoop(bridge, installedPresenter, loopClock(), options);
        presenting.start();
      },
      stopPresenting,
      attachCanvas,
      detachCanvas,
      redraw: () => canvasPresenter?.redraw() ?? false,
      getPresentStats: () => presenting?.stats() ?? null,
      close: () => {
        stopPresenting();
        presenting = null;
        requireAddon().close();
      },
    },
    menu: {
      onCommand: subscribe<MenuCommand>("menu:command", (v) => (isMenuCommand(v) ? v : undefined)),
    },
    dialog: {
      openProjectFolder: () => invoke("dialog:openProjectFolder"),
      pickMediaFiles: () => invoke("dialog:pickMediaFiles"),
      chooseExportFile: (defaultPath) => invoke("dialog:chooseExportFile", defaultPath),
    },
    recent: {
      list: () => invoke("recent:list"),
      add: (path) => invoke("recent:add", path),
      remove: (path) => invoke("recent:remove", path),
      clear: () => invoke("recent:clear"),
    },
    window: {
      setDirty: (dirty) => invoke("window:setDirty", dirty),
      onCloseRequested: (listener) =>
        onCloseRequested((value) => {
          if (value === true) listener();
        }),
      confirmClose: () => invoke("window:confirmClose"),
      toggleFullScreen: () => invoke("window:toggleFullScreen"),
      setTheme: (theme) => invoke("window:setTheme", theme),
      reload: () => invoke("window:reload"),
      toggleDevTools: () => invoke("window:toggleDevTools"),
      inspectElement: (x, y) => invoke("window:inspectElement", x, y),
    },
    edit: {
      run: (action) => invoke("edit:run", action),
    },
    files: {
      pathForFile: (file) => {
        // A caller-supplied value: only a real `File` goes to Electron, and a failure is "no path".
        if (typeof File === "undefined" || !(file instanceof File)) return "";
        try {
          const path = getPathForFile(file);
          return path.length <= MAX_PATH_LENGTH && !path.includes("\u0000") ? path : "";
        } catch {
          return "";
        }
      },
    },
    prefs: {
      get: () => invoke("prefs:get"),
      set: (key, value) => invoke("prefs:set", key, value),
      reset: (keys) => invoke("prefs:reset", [...keys]),
      exportToFile: (only) =>
        only === undefined ? invoke("prefs:export") : invoke("prefs:export", only),
      importFromFile: (only) =>
        only === undefined ? invoke("prefs:import") : invoke("prefs:import", only),
      onChanged: subscribe<PreferenceChanges>("prefs:changed", (v) =>
        typeof v === "object" && v !== null && !Array.isArray(v)
          ? (v as PreferenceChanges)
          : undefined,
      ),
    },
    agents: {
      addFolder: () => invoke("agents:addFolder"),
      removeFolder: (folder) => invoke("agents:removeFolder", folder),
      connectInfo: () => invoke("agents:connectInfo"),
      copySetup: (client) => invoke("agents:copySetup", client),
    },
    shell: {
      openAppFolder: (which) => invoke("shell:openAppFolder", which),
      showItemInFolder: (path) => invoke("shell:showItemInFolder", path),
      openExternal: (url) => invoke("shell:openExternal", url),
    },
  };
}
