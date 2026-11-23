// SPDX-License-Identifier: GPL-3.0-or-later
// Connects the editor to its engine: one `RpcConnection`, and the stores fed from it (docStore
// through `DocSync`, viewStore through `ViewSync`, playbackStore, jobStore). The connection
// address comes from the preload (`window.cutweave.engine`, WP-1L.01); with no engine the editor
// keeps its sample data and `startEngineRuntime` is simply never called.

import {
  type ConnectionHooks,
  type ConnectionInfo,
  DocSync,
  type JobInfo,
  RpcClient,
  RpcConnection,
  type RpcConnectionOptions,
} from "@cutweave/protocol";
import { docSink, type docStore } from "../state/docStore.ts";
import type { jobStore } from "../state/jobStore.ts";
import type { playbackStore } from "../state/playbackStore.ts";
import type { viewStore } from "../state/viewStore.ts";
import { ViewSync, type ViewSyncOptions } from "../state/viewSync.ts";
import type { engineStore } from "./engineStore.ts";

/**
 * The part of `window.cutweave.engine` (apps/desktop `shared/api.ts`) the editor uses.
 * `getConnection` is `null` until the supervisor reports the engine ready.
 */
export interface EngineBridge {
  getConnection(): Promise<(ConnectionInfo & { readonly port?: number }) | null>;
  /** Supervisor status changes (`ready`, `restarting`, ...). Optional: absent in older preloads. */
  onStatus?(
    listener: (status: { readonly state: string; readonly port?: number }) => void,
  ): () => void;
}

/** `window.cutweave.engine` when the editor runs inside the desktop app, else `null`. */
export function bridgeFromWindow(
  target: { cutweave?: { engine?: EngineBridge } } = globalThis as never,
): EngineBridge | null {
  return target.cutweave?.engine ?? null;
}

export interface RuntimeStores {
  readonly doc: typeof docStore;
  readonly view: typeof viewStore;
  readonly playback: typeof playbackStore;
  readonly jobs: typeof jobStore;
  readonly engine: typeof engineStore;
}

export interface EngineRuntime {
  readonly connection: RpcConnection;
  readonly rpc: RpcClient;
  readonly docSync: DocSync;
  readonly viewSync: ViewSync;
  /** Stops listening and closes the connection. */
  stop(): Promise<void>;
}

export interface RuntimeOptions {
  /** Extra connection options (tests inject sockets and timers; WP-1L.03 its reconnect policy). */
  readonly connection?: Partial<RpcConnectionOptions>;
  /** Extra hooks, called after the runtime's own. WP-1L.03 attaches the banner and resend here. */
  readonly hooks?: ConnectionHooks;
  readonly view?: Pick<ViewSyncOptions, "debounceMs" | "timers">;
  readonly onError?: (error: unknown) => void;
}

/**
 * Connects and wires the stores. On the first handshake the stores are filled (document
 * snapshot, view, jobs); after a reconnect the same sources are read again, so the UI converges
 * on the engine's state whatever happened while the socket was down.
 */
export function startEngineRuntime(
  bridge: EngineBridge,
  stores: RuntimeStores,
  options: RuntimeOptions = {},
): EngineRuntime {
  const report = options.onError ?? ((error: unknown) => console.error("[engine]", error));
  // Work cut short by `stop()` is not a failure.
  let stopping = false;
  const onError = (error: unknown) => {
    if (!stopping) report(error);
  };
  const extra = options.hooks;

  const connection: RpcConnection = new RpcConnection({
    resolve: async () => {
      const info = await bridge.getConnection();
      return info === null ? null : { wsUrl: info.wsUrl, token: info.token };
    },
    ...options.connection,
    hooks: {
      ...extra,
      onStateChange(state, previous) {
        stores.engine.setState({ connection: state });
        extra?.onStateChange?.(state, previous);
      },
      async onConnected(hello, context) {
        await Promise.all([sync(), extra?.onConnected?.(hello, context)]);
      },
      async onReconnected(hello, context) {
        await Promise.all([sync(), extra?.onReconnected?.(hello, context)]);
      },
      onError(error) {
        onError(error);
        extra?.onError?.(error);
      },
    },
  });
  const rpc = new RpcClient(connection);

  const docSync = new DocSync(rpc, docSink(stores.doc), { onError });
  const viewSync = new ViewSync({ store: stores.view, rpc, onError, ...options.view });
  let started = false;
  const offs: (() => void)[] = [];

  /** Reads everything the stores mirror. */
  async function sync(): Promise<void> {
    if (!started) {
      started = true;
      offs.push(
        rpc.on("job.progress", (info: JobInfo) => stores.jobs.getState().applyProgress(info)),
        rpc.onUntyped("playback.state", (payload) =>
          stores.playback.getState().applyState(payload),
        ),
      );
      await docSync.start();
    } else {
      await docSync.resync();
    }
    const [, list] = await Promise.all([
      viewSync.restore(),
      rpc.call("job.list", {}).then((out) => out.jobs),
    ]);
    stores.jobs.getState().reset(list);
  }

  // The supervisor knows first when the engine came back (possibly on a new port): do not wait
  // out the backoff. WP-1L.03 owns the full restart behaviour; this is only the nudge.
  const offStatus = bridge.onStatus?.((status) => {
    if (
      status.state === "ready" &&
      (connection.state === "reconnecting" || connection.state === "failed")
    ) {
      connection.reconnectNow();
    }
  });

  stores.engine.setState({ rpc, connection: connection.state });
  connection.connect();

  return {
    connection,
    rpc,
    docSync,
    viewSync,
    async stop() {
      stopping = true;
      offStatus?.();
      for (const off of offs) off();
      docSync.stop();
      await viewSync.stop().catch(onError);
      connection.close();
      stores.engine.setState({ rpc: null, connection: "none" });
    },
  };
}
