// SPDX-License-Identifier: GPL-3.0-or-later
// Builds the `window.cutweave` object from an `ipcRenderer` and the (optional) bridge addon.
// Kept free of `electron` imports so it is unit-testable; `index.ts` wires the real ones.

import { type InvokeChannel, type InvokeChannels, isMenuCommand } from "../ipc.ts";
import type { CutweaveApi, EngineStatus, MenuCommand } from "../shared/api.ts";
import type { BridgeAddon } from "./bridge-addon.ts";

/** Slice of `ipcRenderer` the preload uses. */
export interface IpcRendererLike {
  invoke(channel: string, ...args: unknown[]): Promise<unknown>;
  on(channel: string, listener: (event: unknown, ...args: unknown[]) => void): unknown;
  removeListener(channel: string, listener: (event: unknown, ...args: unknown[]) => void): unknown;
}

export const ERR_BRIDGE_UNAVAILABLE = "ERR_BRIDGE_UNAVAILABLE";

export function createCutweaveApi(ipc: IpcRendererLike, addon: BridgeAddon | null): CutweaveApi {
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

  const requireAddon = (): BridgeAddon => {
    if (addon === null) {
      throw new Error(`${ERR_BRIDGE_UNAVAILABLE}: the cutweave-bridge addon is not loaded`);
    }
    return addon;
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
      open: (ringName) => requireAddon().open(ringName),
      getLatestFrame: (maxPtsFlicks) => requireAddon().getLatestFrame(maxPtsFlicks),
      getClock: () => requireAddon().getClock(),
      close: () => requireAddon().close(),
    },
    menu: {
      onCommand: subscribe<MenuCommand>("menu:command", (v) => (isMenuCommand(v) ? v : undefined)),
    },
    dialog: { openProjectFolder: () => invoke("dialog:openProjectFolder") },
    shell: { showItemInFolder: (path) => invoke("shell:showItemInFolder", path) },
  };
}
