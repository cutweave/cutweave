// SPDX-License-Identifier: GPL-3.0-or-later
// Builds the `window.cutweave` object from an `ipcRenderer` and the (optional) bridge addon.
// Kept free of `electron` imports so it is unit-testable; `index.ts` wires the real ones.

import type { CutweaveApi, EngineStatus, MenuCommand } from "../shared/api.ts";
import { type InvokeChannel, type InvokeChannels, isMenuCommand } from "../shared/ipc-contract.ts";
import type { BridgeAddon } from "./bridge-addon.ts";
import {
  browserLoopClock,
  currentFramePresenter,
  type LoopClock,
  PresentLoop,
} from "./present-loop.ts";

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

  return {
    app: { getInfo: () => invoke("app:getInfo") },
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
        presenting = new PresentLoop(bridge, currentFramePresenter(), loopClock(), options);
        presenting.start();
      },
      stopPresenting,
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
    },
    recent: {
      list: () => invoke("recent:list"),
      add: (path) => invoke("recent:add", path),
      remove: (path) => invoke("recent:remove", path),
    },
    window: {
      setDirty: (dirty) => invoke("window:setDirty", dirty),
      onCloseRequested: (listener) =>
        onCloseRequested((value) => {
          if (value === true) listener();
        }),
      confirmClose: () => invoke("window:confirmClose"),
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
    shell: {
      showItemInFolder: (path) => invoke("shell:showItemInFolder", path),
      openExternal: (url) => invoke("shell:openExternal", url),
    },
  };
}
