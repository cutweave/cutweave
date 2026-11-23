// SPDX-License-Identifier: GPL-3.0-or-later
import type { ConnectionState, RpcClient } from "@cutweave/protocol";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

export interface EngineStoreState {
  /** `none` while the editor runs without an engine (plain `vite` dev, Storybook). */
  connection: ConnectionState | "none";
  /** The typed client of the live connection; `null` without an engine. */
  rpc: RpcClient | null;
}

export function createEngineStore() {
  return createStore<EngineStoreState>()(() => ({ connection: "none", rpc: null }));
}

/** What the hooks and panels read; `startEngineRuntime` is the only writer. */
export const engineStore = createEngineStore();

export function useEngineStore<T>(selector: (state: EngineStoreState) => T): T {
  return useStore(engineStore, selector);
}

/** The connection state, for banners and disabled controls. */
export function useConnectionState(): EngineStoreState["connection"] {
  return useEngineStore((s) => s.connection);
}

/** Whether an engine is attached and the connection is up. */
export function useEngineReady(): boolean {
  return useEngineStore((s) => s.connection === "ready");
}
