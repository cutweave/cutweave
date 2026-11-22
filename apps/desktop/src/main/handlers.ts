// SPDX-License-Identifier: GPL-3.0-or-later
// Implementations of the allowlisted IPC channels (src/ipc.ts), with Electron injected.

import type { InvokeHandlers } from "../ipc.ts";
import type { AppInfo, EngineConnection, EngineStatus } from "../shared/api.ts";

export interface HandlerDeps {
  appInfo: AppInfo;
  /** Current engine status. */
  engineStatus(): EngineStatus;
  /** Port and UI token of the supervised engine; null when none is ready. */
  engineCredentials(): { port: number; token: string } | null;
  chooseFolder(): Promise<string | null>;
  showItemInFolder(path: string): void;
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
  };
}
