// SPDX-License-Identifier: GPL-3.0-or-later
// What the "This computer" pages read from the engine while they are open (WP-1D.08): the
// caches' usage and limits (`media.cache_status`) and the GPU and hardware encoders found
// (`media.capabilities`). Read-only and polled gently; changes go through the preferences store
// (the runtime sends them) or, for "Clear", straight to the engine from the row's button.

import type {
  CacheKind,
  MediaCacheStatusOutput,
  MediaCapabilitiesOutput,
} from "@cutweave/protocol";
import { useEffect } from "react";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";
import { engineStore } from "../engine/engineStore.ts";

interface MachineState {
  readonly status: MediaCacheStatusOutput | null;
  readonly capabilities: MediaCapabilitiesOutput | null;
}

export const machineStore = createStore<MachineState>()(() => ({
  status: null,
  capabilities: null,
}));

/** How often an open page asks again (the GPU may still be opening, caches fill as media is analysed). */
const POLL_MS = 2000;

/** Reads the caches' status now. A failed read leaves what was shown. */
export async function refreshCacheStatus(): Promise<void> {
  const rpc = engineStore.getState().rpc;
  if (rpc === null) return;
  try {
    machineStore.setState({ status: await rpc.call("media.cache_status", {}) });
  } catch {
    // The engine is going away or restarting: the next poll finds it again.
  }
}

/** Reads the engine's capabilities now. */
export async function refreshCapabilities(): Promise<void> {
  const rpc = engineStore.getState().rpc;
  if (rpc === null) return;
  try {
    machineStore.setState({ capabilities: await rpc.call("media.capabilities", {}) });
  } catch {
    // As above.
  }
}

/** Empties these caches (UI credential); resolves to whether the engine did it. */
export async function clearCaches(kinds: readonly CacheKind[]): Promise<boolean> {
  const rpc = engineStore.getState().rpc;
  if (rpc === null) return false;
  try {
    await rpc.call("media.clear_cache", { kinds: [...kinds] });
    return true;
  } catch {
    return false;
  } finally {
    await refreshCacheStatus();
  }
}

/** Polls `refresh` while the calling component is mounted and an engine is connected. */
function usePolling(refresh: () => Promise<void>): void {
  const connection = useStore(engineStore, (s) => s.connection);
  useEffect(() => {
    if (connection !== "ready") return;
    void refresh();
    const id = window.setInterval(() => void refresh(), POLL_MS);
    return () => window.clearInterval(id);
  }, [connection, refresh]);
}

/** The caches' status, kept fresh while mounted; `null` until the engine has answered. */
export function useCacheStatus(): MediaCacheStatusOutput | null {
  usePolling(refreshCacheStatus);
  return useStore(machineStore, (s) => s.status);
}

/** The engine's capabilities (GPU in use, hardware encoders), kept fresh while mounted. */
export function useCapabilities(): MediaCapabilitiesOutput | null {
  usePolling(refreshCapabilities);
  return useStore(machineStore, (s) => s.capabilities);
}

/** `1.5 GB`, `320 MB`, `12 KB`; binary sizes under the names people know. */
export function formatBytes(bytes: number): string {
  const KB = 1024;
  const MB = KB * 1024;
  const GB = MB * 1024;
  if (bytes >= GB) {
    const gb = bytes / GB;
    return `${gb >= 10 ? Math.round(gb) : Math.round(gb * 10) / 10} GB`;
  }
  if (bytes >= MB) return `${Math.round(bytes / MB)} MB`;
  return `${Math.max(1, Math.round(bytes / KB))} KB`;
}
