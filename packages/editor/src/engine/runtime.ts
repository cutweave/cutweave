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
import type { agentsStore } from "../agents/agentsStore.ts";
import type { binStore } from "../media/binStore.ts";
import type { mediaStore } from "../media/mediaStore.ts";
import type { prefsStore } from "../prefs/prefsStore.ts";
import { parseRingAnnouncement } from "../preview/ringPreview.ts";
import type { sessionStore } from "../session/sessionStore.ts";
import { type OpenNotice, SessionSync } from "../session/sessionSync.ts";
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
  /** The media bin's search text, cleared when another project opens (WP-1M.12); optional. */
  readonly bin?: typeof binStore;
  readonly engine: typeof engineStore;
  /** The app preferences (WP-1M.11); the saving rows go to the engine. Optional for tests. */
  readonly prefs?: typeof prefsStore;
  /** Project session and undo stacks (WP-1M.07); optional so older callers need not pass it. */
  readonly session?: typeof sessionStore;
  /** The agent connections open on the engine (WP-1J.05); optional for tests. */
  readonly agents?: typeof agentsStore;
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
  /** An agent's or a plugin's project open opened, was refused or failed (ADR-0029). */
  readonly onOpenNotice?: (notice: OpenNotice) => void;
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
          doc: stores.doc,
          onError,
          ...(options.onAutosaveFailed && { onAutosaveFailed: options.onAutosaveFailed }),
          ...(options.onOpenNotice && { onOpenNotice: options.onOpenNotice }),
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
    // How this editor wants its engine to save (File > Settings… > Saving), for every project.
    if (!mismatch) {
      await sendAutosave();
      await sendMachineSettings();
      await sendPreviewQuality();
    }
    // The preview frame ring of this engine launch (WP-1M.04); `null` when it publishes none.
    stores.engine.setState({ previewRing: parseRingAnnouncement(hello.shm) });
    try {
      await syncDocument();
    } finally {
      // Never re-send to an engine of another schema; `onSchemaMismatch` rejected them.
      if (!mismatch) connection.flushOutbox();
    }
    const [, list] = await Promise.all([
      Promise.all([viewSync.restore(), sessionSync?.refreshStacks(), refreshAgents()]),
      rpc.call("job.list", {}).then((out) => out.jobs),
    ]);
    stores.jobs.getState().reset(list);
    if (restarted) await showRestartNotice();
  }

  /** The agent connections open now (WP-1J.05): a new baseline after every handshake. */
  async function refreshAgents(): Promise<void> {
    if (stores.agents === undefined) return;
    try {
      const out = await rpc.call("project.connections", {});
      stores.agents.getState().reset(out.connections);
    } catch (error) {
      // An engine from before WP-1J.05 has no such command: no list, nothing else breaks.
      onError(error);
    }
  }

  /** Tells the engine the app's autosave preference (`project.set_autosave`, arch §17). */
  async function sendAutosave(): Promise<void> {
    const values = stores.prefs?.getState().values;
    if (values === undefined) return;
    try {
      await rpc.call("project.set_autosave", {
        enabled: values["saving.autosave"],
        interval_s: values["saving.interval"],
        keep_backups: values["saving.backups"],
      });
    } catch (error) {
      onError(error);
    }
  }

  /**
   * Tells the engine whether the preview uses proxies (`view.set_preview_quality`, WP-2F.02;
   * the transport bar's toggle, preference `playback.proxies`). The answer is the playback
   * state, which says what the frame on screen shows now. An engine without a transport has
   * nothing to set.
   */
  async function sendPreviewQuality(): Promise<void> {
    const values = stores.prefs?.getState().values;
    if (values === undefined) return;
    // The transport's `view.*` commands are not in the generated schema.
    const call = rpc.call.bind(rpc) as unknown as (
      method: string,
      params: Record<string, unknown>,
    ) => Promise<unknown>;
    try {
      const state = await call("view.set_preview_quality", {
        use_proxies: values["playback.proxies"],
      });
      stores.playback.getState().applyState(state);
    } catch (error) {
      if (!(error instanceof RpcError && error.code === "not_found")) onError(error);
    }
  }

  /**
   * Tells the engine this computer's cache and hardware preferences (`media.set_decode_policy`,
   * `media.set_cache_config`; UI-only commands, WP-1D.08) and how it makes proxies
   * (`media.set_proxy_config`, WP-2F.03b; first, so no proxy starts at another size). The cache
   * folder is not sent: main passes it as `--cache-dir` when it starts the engine.
   */
  async function sendMachineSettings(): Promise<void> {
    const values = stores.prefs?.getState().values;
    if (values === undefined) return;
    try {
      await rpc.call("media.set_proxy_config", {
        size: values["media.proxy_size"],
        jobs: values["performance.proxy_jobs"],
      });
    } catch (error) {
      // An engine that makes no proxies (no media backend) has nothing to set.
      if (!(error instanceof RpcError && error.code === "unsupported")) onError(error);
    }
    try {
      await rpc.call("media.set_decode_policy", { hw: values["performance.hw_decode"] });
      await rpc.call("media.set_cache_config", {
        limits: {
          thumbnails: values["cache.thumbnails_limit"],
          waveforms: values["cache.waveforms_limit"],
          proxies: values["cache.proxies_limit"],
        },
        frame_cache_bytes: values["performance.frame_memory"],
      });
    } catch (error) {
      onError(error);
    }
  }

  async function syncDocument(): Promise<void> {
    if (!started) {
      started = true;
      sessionSync?.start();
      offs.push(
        rpc.on("job.progress", (info: JobInfo) => stores.jobs.getState().applyProgress(info)),
        rpc.on("media.analysis_failed", (e) => stores.media?.getState().applyAnalysisFailed(e)),
        rpc.on("media.indexed", (e) => stores.media?.getState().applyIndexed(e)),
        rpc.on("project.opened", () => stores.bin?.getState().clearSearch()),
        rpc.on("project.closed", () => stores.bin?.getState().clearSearch()),
        rpc.on("project.connection_opened", (c) => stores.agents?.getState().opened(c)),
        rpc.on("project.connection_closed", (c) => stores.agents?.getState().closed(c)),
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

  // A change in Settings goes to the engine at once; while disconnected, the next handshake
  // sends it.
  if (stores.prefs !== undefined) {
    offs.push(
      stores.prefs.subscribe((state, previous) => {
        const changed =
          state.values["saving.autosave"] !== previous.values["saving.autosave"] ||
          state.values["saving.interval"] !== previous.values["saving.interval"] ||
          state.values["saving.backups"] !== previous.values["saving.backups"];
        if (changed && connection.state === "ready") void sendAutosave();
        const machineChanged =
          state.values["performance.hw_decode"] !== previous.values["performance.hw_decode"] ||
          state.values["performance.frame_memory"] !==
            previous.values["performance.frame_memory"] ||
          state.values["cache.thumbnails_limit"] !== previous.values["cache.thumbnails_limit"] ||
          state.values["cache.waveforms_limit"] !== previous.values["cache.waveforms_limit"] ||
          state.values["cache.proxies_limit"] !== previous.values["cache.proxies_limit"] ||
          state.values["media.proxy_size"] !== previous.values["media.proxy_size"] ||
          state.values["performance.proxy_jobs"] !== previous.values["performance.proxy_jobs"];
        if (machineChanged && connection.state === "ready") void sendMachineSettings();
        if (
          state.values["playback.proxies"] !== previous.values["playback.proxies"] &&
          connection.state === "ready"
        ) {
          void sendPreviewQuality();
        }
      }),
    );
  }

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
      stores.engine.setState({
        rpc: null,
        connection: "none",
        supervisor: null,
        notice: null,
        previewRing: undefined,
      });
    },
  };
}
