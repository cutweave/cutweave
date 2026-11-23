// SPDX-License-Identifier: GPL-3.0-or-later
// Connects the editor to its engine: one `RpcConnection`, and the stores fed from it (docStore
// through `DocSync`, viewStore through `ViewSync`, playbackStore, jobStore). The connection
// address comes from the preload (`window.cutweave.engine`, WP-1L.01); with no engine the editor
// keeps its sample data and `startEngineRuntime` is simply never called.
//
// Engine restarts (WP-1L.03, renderer half). Electron main supervises the engine and reports
// its state (`onStatus`); this runtime follows it:
// - Unanswered commands are kept with their `request_id` (`retainUnacked`) and sent again only
//   after the document snapshot of the new handshake (`autoFlush: false`): the engine
//   deduplicates them through its journal, so each applies exactly once. Calls bound to the lost
//   socket's transaction (a gesture) are rejected instead; their owner starts over.
// - While main restarts the engine the connection waits however long it takes and reconnects as
//   soon as main says `ready`, with the new port and token (`getConnection()` again; the token
//   is per engine launch). A higher `generation` means the engine restarted: after the resync
//   the "Engine restarted — recovered" notice shows what main restored from the journal.
// - Main `failed` (it gave up restarting) or a schema hash that differs from this build's:
//   editing is blocked with a message, and waiting commands are rejected.

import {
  type ConnectionHooks,
  type ConnectionInfo,
  DocSync,
  type JobInfo,
  RpcClient,
  RpcConnection,
  type RpcConnectionOptions,
  RpcError,
  type RpcServerHello,
  SCHEMA_HASH,
  type SupervisorHint,
  supervisedPolicy,
} from "@cutweave/protocol";
import type { mediaStore } from "../media/mediaStore.ts";
import type { sessionStore } from "../session/sessionStore.ts";
import { SessionSync } from "../session/sessionSync.ts";
import { docSink, type docStore } from "../state/docStore.ts";
import type { jobStore } from "../state/jobStore.ts";
import type { playbackStore } from "../state/playbackStore.ts";
import type { viewStore } from "../state/viewStore.ts";
import { ViewSync, type ViewSyncOptions } from "../state/viewSync.ts";
import type { EngineBlock, engineStore, SupervisorStatus } from "./engineStore.ts";

/**
 * The part of `window.cutweave.engine` (apps/desktop `shared/api.ts`) the editor uses.
 * `getConnection` is `null` until the supervisor reports the engine ready.
 */
export interface EngineBridge {
  getConnection(): Promise<
    (ConnectionInfo & { readonly port?: number; readonly generation?: number }) | null
  >;
  /** The supervisor's current status. Optional: absent in older preloads and tests. */
  getStatus?(): Promise<SupervisorStatus>;
  /** Supervisor status changes (`ready`, `restarting`, ...). Optional like `getStatus`. */
  onStatus?(listener: (status: SupervisorStatus) => void): () => void;
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
  /** Asset analysis status (`media.analysis_failed`, `media.indexed`); optional for tests. */
  readonly media?: typeof mediaStore;
  readonly engine: typeof engineStore;
  /** Project session and undo stacks (WP-1M.07); optional so older callers need not pass it. */
  readonly session?: typeof sessionStore;
}

export interface EngineRuntime {
  readonly connection: RpcConnection;
  readonly rpc: RpcClient;
  readonly docSync: DocSync;
  readonly viewSync: ViewSync;
  /** Feeds the session store; `null` when none was passed. */
  readonly sessionSync: SessionSync | null;
  /** Stops listening and closes the connection. */
  stop(): Promise<void>;
}

export interface RuntimeOptions {
  /** Extra connection options (tests inject sockets, timers and a reconnect policy). */
  readonly connection?: Partial<RpcConnectionOptions>;
  /** Extra hooks, called after the runtime's own. */
  readonly hooks?: ConnectionHooks;
  readonly view?: Pick<ViewSyncOptions, "debounceMs" | "timers">;
  readonly onError?: (error: unknown) => void;
  /** `project.autosave_failed`: show the engine's message (a toast). */
  readonly onAutosaveFailed?: (message: string) => void;
}

/** What the supervisor's state means for reconnecting (`supervisedPolicy`). */
export function supervisorHint(status: SupervisorStatus | null): SupervisorHint {
  switch (status?.state) {
    case undefined:
      return "unknown";
    case "ready":
      return "up";
    case "starting":
    case "crashed":
    case "restarting":
      return "restarting";
    default:
      // failed, stopped, unavailable
      return "down";
  }
}

const STOPPED_MESSAGE = "the engine stopped before it confirmed this change";

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

  /** The supervisor's latest report. */
  let status: SupervisorStatus | null = null;
  /** Engine launch of the address `resolve()` returned last, and of the last handshake. */
  let resolvedGeneration: number | undefined;
  let connectedGeneration: number | undefined;
  let noticeId = 0;

  const setBlock = (blocked: EngineBlock | null) => stores.engine.setState({ blocked });
  const rejectWaiting = (message: string) =>
    connection.outbox.rejectWhere(new RpcError("unsupported", message));

  const connection: RpcConnection = new RpcConnection({
    resolve: async () => {
      const info = await bridge.getConnection();
      if (info === null) return null;
      resolvedGeneration = info.generation;
      return { wsUrl: info.wsUrl, token: info.token };
    },
    reconnect: supervisedPolicy(() => supervisorHint(status)),
    retainUnacked: true,
    autoFlush: false,
    ...options.connection,
    hooks: {
      ...extra,
      onStateChange(state, previous) {
        stores.engine.setState({ connection: state });
        extra?.onStateChange?.(state, previous);
      },
      onSchemaMismatch(hello) {
        setBlock({ kind: "schema-mismatch", engineVersion: hello.engine_version });
        rejectWaiting("this editor and the engine use different command schemas");
        extra?.onSchemaMismatch?.(hello);
      },
      async onConnected(hello, context) {
        connectedGeneration = resolvedGeneration;
        await Promise.all([afterHandshake(hello, false), extra?.onConnected?.(hello, context)]);
      },
      async onReconnected(hello, context) {
        const restarted =
          resolvedGeneration !== undefined &&
          connectedGeneration !== undefined &&
          resolvedGeneration > connectedGeneration;
        connectedGeneration = resolvedGeneration;
        await Promise.all([
          afterHandshake(hello, restarted),
          extra?.onReconnected?.(hello, context),
        ]);
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
  const sessionSync =
    stores.session === undefined
      ? null
      : new SessionSync({
          store: stores.session,
          rpc,
          onError,
          ...(options.onAutosaveFailed && { onAutosaveFailed: options.onAutosaveFailed }),
        });
  let started = false;
  const offs: (() => void)[] = [];

  /**
   * After every handshake: the document snapshot first, then the commands still waiting (their
   * patches land on top of it), then the view and the jobs. A restart ends with the notice.
   */
  async function afterHandshake(hello: RpcServerHello, restarted: boolean): Promise<void> {
    const mismatch = hello.schema_hash !== SCHEMA_HASH;
    if (!mismatch && stores.engine.getState().blocked?.kind === "schema-mismatch") {
      setBlock(null);
    }
    // The project status first: the events that follow (dirty, saved) are newer than the hello.
    sessionSync?.applyHello(hello);
    try {
      await syncDocument();
    } finally {
      // Never re-send to an engine of another schema; `onSchemaMismatch` rejected them.
      if (!mismatch) connection.flushOutbox();
    }
    const [, list] = await Promise.all([
      Promise.all([viewSync.restore(), sessionSync?.refreshStacks()]),
      rpc.call("job.list", {}).then((out) => out.jobs),
    ]);
    stores.jobs.getState().reset(list);
    if (restarted) await showRestartNotice();
  }

  async function syncDocument(): Promise<void> {
    if (!started) {
      started = true;
      sessionSync?.start();
      offs.push(
        rpc.on("job.progress", (info: JobInfo) => stores.jobs.getState().applyProgress(info)),
        rpc.on("media.analysis_failed", (e) => stores.media?.getState().applyAnalysisFailed(e)),
        rpc.on("media.indexed", (e) => stores.media?.getState().applyIndexed(e)),
        rpc.onUntyped("playback.state", (payload) =>
          stores.playback.getState().applyState(payload),
        ),
      );
      await docSync.start();
    } else {
      await docSync.resync();
    }
  }

  /** What main restored for the engine launch the editor is now connected to. */
  async function showRestartNotice(): Promise<void> {
    let current = status;
    if (current?.generation !== connectedGeneration && bridge.getStatus) {
      current = await bridge.getStatus().catch(() => current);
    }
    const recovery = current?.generation === connectedGeneration ? current?.recovery : undefined;
    stores.engine.setState({ notice: { id: ++noticeId, recovery: recovery ?? null } });
  }

  /** Follows the supervisor: block and unblock, and reconnect the moment the engine is back. */
  function onStatus(next: SupervisorStatus): void {
    status = next;
    stores.engine.setState({ supervisor: next });
    if (next.state === "failed") {
      setBlock({ kind: "engine-failed", message: next.message ?? null });
      rejectWaiting(STOPPED_MESSAGE);
      return;
    }
    if (next.state !== "ready") return;
    if (stores.engine.getState().blocked?.kind === "engine-failed") setBlock(null);
    const newer =
      next.generation !== undefined &&
      connectedGeneration !== undefined &&
      next.generation > connectedGeneration;
    // Do not wait out a backoff, and leave a socket to an engine that has been replaced.
    if (
      connection.state === "reconnecting" ||
      connection.state === "failed" ||
      (newer && connection.state === "ready")
    ) {
      connection.reconnectNow();
    }
  }

  const offStatus = bridge.onStatus?.(onStatus);
  bridge
    .getStatus?.()
    .then((initial) => {
      // An event may have overtaken the first read.
      if (status === null) onStatus(initial);
    })
    .catch(onError);

  stores.engine.setState({ rpc, connection: connection.state });
  connection.connect();

  return {
    connection,
    rpc,
    docSync,
    viewSync,
    sessionSync,
    async stop() {
      stopping = true;
      offStatus?.();
      for (const off of offs) off();
      docSync.stop();
      sessionSync?.stop();
      await viewSync.stop().catch(onError);
      connection.close();
      stores.engine.setState({ rpc: null, connection: "none", supervisor: null, notice: null });
    },
  };
}
