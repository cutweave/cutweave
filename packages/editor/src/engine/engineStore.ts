// SPDX-License-Identifier: GPL-3.0-or-later
import type { ConnectionState, RpcClient } from "@cutweave/protocol";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

/** Journal recovery the supervisor ran after restarting the engine (desktop `EngineRecovery`). */
export interface EngineRecoveryView {
  readonly project: string;
  readonly applied: number;
  readonly damaged: boolean;
}

/**
 * What the supervisor (Electron main, WP-1L.03) reports about the engine process: the slice of
 * the desktop `EngineStatus` the editor reads. `generation` grows with every engine launch.
 */
export interface SupervisorStatus {
  readonly state: string;
  readonly port?: number;
  readonly generation?: number;
  readonly message?: string;
  readonly recovery?: EngineRecoveryView;
  readonly lastError?: { readonly code: string; readonly message: string };
}

/** Why editing is off: the engine is gone for good, or it speaks another schema. */
export type EngineBlock =
  | { readonly kind: "engine-failed"; readonly message: string | null }
  | { readonly kind: "schema-mismatch"; readonly engineVersion: string };

/** "Engine restarted — recovered", shown for a moment after the editor is back in sync. */
export interface EngineNotice {
  /** Grows with every notice, so the same text can be shown twice. */
  readonly id: number;
  /** What main restored from the journal; `null` when no project was open (nothing journaled). */
  readonly recovery: EngineRecoveryView | null;
}

export interface EngineStoreState {
  /** `none` while the editor runs without an engine (plain `vite` dev, Storybook). */
  connection: ConnectionState | "none";
  /** The typed client of the live connection; `null` without an engine. */
  rpc: RpcClient | null;
  /** The supervisor's latest report; `null` without one. */
  supervisor: SupervisorStatus | null;
  /** Set while edits are refused (a blocking message is shown). */
  blocked: EngineBlock | null;
  /** The current restart notice, if any. */
  notice: EngineNotice | null;
}

export const initialEngineState: EngineStoreState = {
  connection: "none",
  rpc: null,
  supervisor: null,
  blocked: null,
  notice: null,
};

export function createEngineStore() {
  return createStore<EngineStoreState>()(() => ({ ...initialEngineState }));
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

/** The status the top bar shows, from the connection, the supervisor and any block. */
export type EngineIndicator =
  | "none"
  | "connecting"
  | "ready"
  | "reconnecting"
  | "stopped"
  | "mismatch";

export function engineIndicator(state: EngineStoreState): EngineIndicator {
  if (state.blocked?.kind === "schema-mismatch") return "mismatch";
  if (state.blocked?.kind === "engine-failed") return "stopped";
  if (state.supervisor?.state === "unavailable") return "none";
  switch (state.connection) {
    case "none":
      return "none";
    case "ready":
      return "ready";
    case "reconnecting":
      return "reconnecting";
    case "failed":
    case "closed":
      return "stopped";
    default:
      return "connecting";
  }
}
