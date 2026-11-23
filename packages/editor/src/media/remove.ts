// SPDX-License-Identifier: GPL-3.0-or-later
// Removing an asset from the project (`media.remove`; the file on disk stays). The engine refuses
// while clips still use the asset (`Invalid`); its message is shown as it is, because it names
// what to do (arch §5.1: no client-side duplicate validation).
import { type MediaId, RpcError } from "@cutweave/protocol";
import type { StoreApi } from "zustand";
import type { EngineStoreState } from "../engine/engineStore.ts";
import type { ToastStore } from "../state/toastStore.ts";
import type { Translate } from "../timeline/errors.ts";

export interface RemoveDeps {
  readonly engine: StoreApi<EngineStoreState>;
  readonly toasts: StoreApi<ToastStore>;
  readonly t: Translate;
}

/** Removes `id`; a refusal becomes a toast carrying the engine's message. Resolves to success. */
export async function removeMedia(deps: RemoveDeps, id: MediaId, name: string): Promise<boolean> {
  const rpc = deps.engine.getState().rpc;
  if (rpc === null) return false;
  try {
    await rpc.call("media.remove", { id });
    return true;
  } catch (error) {
    deps.toasts.getState().push({
      tone: "danger",
      title: deps.t("media.remove.failedTitle", { name }),
      message: error instanceof RpcError ? error.message : String(error),
    });
    return false;
  }
}
