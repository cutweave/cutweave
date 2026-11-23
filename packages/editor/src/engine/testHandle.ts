// SPDX-License-Identifier: GPL-3.0-or-later
// The editor's e2e handle (WP-1L.03): Playwright drives the editor's own engine connection
// through `window.__cutweaveEditor`, so the reconnect tests exercise the real outbox, resend and
// resync. Installed only when the desktop app says its test hooks are on (unpackaged builds with
// CUTWEAVE_TEST_HOOKS=1, `AppInfo.testHooks`); it adds no power the page lacks (the page can
// already open its own engine socket with `getConnection()`).

import { needsRequestId, RpcError } from "@cutweave/protocol";
import { type EngineIndicator, engineIndicator } from "./engineStore.ts";
import { editBlockedReason } from "./hooks.ts";
import type { EngineRuntime, RuntimeStores } from "./runtime.ts";

export interface EditorTestState {
  readonly connection: string;
  readonly indicator: EngineIndicator;
  readonly supervisorState: string | null;
  readonly generation: number | null;
  readonly blocked: string | null;
  readonly noticeId: number | null;
  readonly recoveredApplied: number | null;
  readonly docRev: number;
  /** Methods of the calls still waiting for an answer, in call order. */
  readonly outbox: readonly string[];
}

export interface EditorTestHandle {
  /** A call over the editor's connection, as a command or query from the UI would make it. */
  call(method: string, params?: Record<string, unknown>): Promise<unknown>;
  state(): EditorTestState;
}

declare global {
  var __cutweaveEditor: EditorTestHandle | undefined;
}

/** `window.cutweave.app` (apps/desktop `shared/api.ts`), when the preload provides it. */
export interface AppBridge {
  getInfo(): Promise<{ readonly testHooks?: boolean }>;
}

export function appBridgeFromWindow(
  target: { cutweave?: { app?: AppBridge } } = globalThis as never,
): AppBridge | null {
  return target.cutweave?.app ?? null;
}

export function createTestHandle(runtime: EngineRuntime, stores: RuntimeStores): EditorTestHandle {
  return {
    call(method, params = {}) {
      const blocked = needsRequestId(method) ? editBlockedReason() : null;
      if (blocked !== null) return Promise.reject(new RpcError("unsupported", blocked));
      return runtime.connection.request(method, params);
    },
    state() {
      const engine = stores.engine.getState();
      return {
        connection: engine.connection,
        indicator: engineIndicator(engine),
        supervisorState: engine.supervisor?.state ?? null,
        generation: engine.supervisor?.generation ?? null,
        blocked: engine.blocked?.kind ?? null,
        noticeId: engine.notice?.id ?? null,
        recoveredApplied: engine.notice?.recovery?.applied ?? null,
        docRev: stores.doc.getState().rev,
        outbox: runtime.connection.outbox.views().map((v) => v.method),
      };
    },
  };
}

/** Installs the handle when the desktop app reports its test hooks; otherwise does nothing. */
export async function installTestHandleIfEnabled(
  app: AppBridge | null,
  runtime: EngineRuntime,
  stores: RuntimeStores,
): Promise<void> {
  const info = await app?.getInfo().catch(() => null);
  if (info?.testHooks === true) {
    globalThis.__cutweaveEditor = createTestHandle(runtime, stores);
  }
}
