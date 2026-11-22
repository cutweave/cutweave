// SPDX-License-Identifier: GPL-3.0-or-later
// Implementations of the allowlisted IPC channels (src/main/ipc.ts), with Electron injected.
// Arguments arrive already sender-checked and zod-validated.

import type { AppInfo, EngineConnection, EngineStatus } from "../shared/api.ts";
import type { InvokeHandlers } from "./ipc.ts";
import { parseExternalUrl } from "./origins.ts";

export interface HandlerDeps {
  appInfo: AppInfo;
  /** Current engine status. */
  engineStatus(): EngineStatus;
  /** Port and UI token of the supervised engine; null when none is ready. */
  engineCredentials(): { port: number; token: string } | null;
  chooseFolder(): Promise<string | null>;
  showItemInFolder(path: string): void;
  /** `shell.openExternal`; only ever called with a validated `https:` URL. */
  openExternal(url: string): Promise<void>;
}

export function createHandlers(deps: HandlerDeps): InvokeHandlers {
  return {
    "app:getInfo": () => deps.appInfo,
    "engine:getStatus": () => deps.engineStatus(),
    "engine:getConnection": (): EngineConnection | null => {
      const creds = deps.engineCredentials();
      if (creds === null) return null;
      return {
        port: creds.port,
        token: creds.token,
        httpUrl: `http://127.0.0.1:${creds.port}`,
        wsUrl: `ws://127.0.0.1:${creds.port}/rpc`,
      };
    },
    "dialog:openProjectFolder": () => deps.chooseFolder(),
    "shell:showItemInFolder": (path) => {
      deps.showItemInFolder(path);
    },
    "shell:openExternal": async (url) => {
      // Re-check at the point of use (defense in depth; the schema already did).
      const safe = parseExternalUrl(url);
      if (safe === null) throw new Error("refusing to open a non-https URL");
      await deps.openExternal(safe);
    },
  };
}
