// SPDX-License-Identifier: GPL-3.0-or-later
// `RpcConnection`: the WebSocket `Transport` (arch §6, rpc.md). It owns the socket, correlates
// responses by JSON-RPC id, routes event notifications, performs the `rpc.hello` and
// `rpc.subscribe` handshake and runs the reconnect state machine (`connection-state.ts`).
//
// WP-1L.03 completes the engine-restart behaviour (re-sending un-acked commands, banner) through
// the hook points below: `hooks.onDisconnected`, `hooks.onReconnected`, the `outbox` and
// `retainUnacked` / `autoFlush`. Nothing here re-sends a command by itself except through them.

import {
  backoffPolicy,
  type ConnectionEvent,
  ConnectionMachine,
  type ConnectionState,
  type ReconnectPolicy,
  type SocketLoss,
  type StateListener,
} from "./connection-state.ts";
import { SCHEMA_HASH } from "./generated/schema-hash.ts";
import type { ProjectStatus } from "./generated/types.ts";
import { needsRequestId } from "./mutating.ts";
import { Outbox, type OutboxEntry, type OutboxEntryView } from "./outbox.ts";
import { uuidv7 } from "./request-id.ts";
import { type RequestMeta, RpcError, type Transport } from "./rpc.ts";

/** Answer of `rpc.hello` (rpc.md). */
export interface RpcServerHello {
  readonly engine_version: string;
  /** Compare with the client's generated {@link SCHEMA_HASH}. */
  readonly schema_hash: string;
  readonly rev: number;
  readonly committed_rev: number;
  /** The actor the credential maps to (`user`, `agent:<label>`). */
  readonly actor: string;
  readonly project: ProjectStatus;
  readonly shm?: unknown;
}

/** Where and how to reach the engine, as the preload's `engine.getConnection()` reports it. */
export interface ConnectionInfo {
  /** `ws://127.0.0.1:<port>/rpc`. */
  readonly wsUrl: string;
  /** The per-launch credential. Sent only as a WebSocket subprotocol: never in a URL. */
  readonly token: string;
}

/** The slice of the WebSocket API the connection uses (browsers, Electron, Node 24, test fakes). */
export interface WebSocketLike {
  onopen: ((event: unknown) => void) | null;
  onmessage: ((event: { readonly data: unknown }) => void) | null;
  onclose: ((event: { readonly code: number; readonly reason: string }) => void) | null;
  onerror: ((event: unknown) => void) | null;
  send(data: string): void;
  close(code?: number, reason?: string): void;
}

export type SocketFactory = (url: string, protocols: string[]) => WebSocketLike;

const defaultSocketFactory: SocketFactory = (url, protocols) =>
  new WebSocket(url, protocols) as unknown as WebSocketLike;

/** The WebSocket subprotocol that carries the token where headers cannot be set (rpc.md). */
export function tokenSubprotocol(token: string): string {
  return `cutweave.token.${token}`;
}

export interface DisconnectInfo extends SocketLoss {
  /** Whether the retry policy will try again. */
  readonly willRetry: boolean;
  /** Calls still waiting for an answer, with the `request_id` to re-send them under. */
  readonly unacked: readonly OutboxEntryView[];
}

export interface ReconnectContext {
  /** Everything still waiting: re-sent by `flushOutbox()` (automatic unless `autoFlush: false`). */
  readonly outbox: Outbox;
  /** The connection, for calls made while handling the hook. */
  readonly connection: RpcConnection;
}

export interface ConnectionHooks {
  onStateChange?(state: ConnectionState, previous: ConnectionState): void;
  /** The socket was lost while `ready` (not on a failed attempt). */
  onDisconnected?(info: DisconnectInfo): void;
  /** The first handshake finished. */
  onConnected?(hello: RpcServerHello, context: ReconnectContext): void | Promise<void>;
  /** A later handshake finished: subscriptions are restored, nothing re-sent yet when `autoFlush` is off. */
  onReconnected?(hello: RpcServerHello, context: ReconnectContext): void | Promise<void>;
  /** The engine's schema hash differs from this build's generated one. */
  onSchemaMismatch?(hello: RpcServerHello): void;
  /** Failures nobody awaits: a throwing event handler or hook, a failed `rpc.subscribe`. */
  onError?(error: unknown): void;
}

export interface Timers {
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

export interface RpcConnectionOptions {
  /**
   * Resolves the engine address on every (re)connect: after an engine restart the port and
   * token may be new. `null` means "not available yet" and counts as a failed attempt.
   */
  readonly resolve: () => ConnectionInfo | null | Promise<ConnectionInfo | null>;
  readonly createSocket?: SocketFactory;
  readonly timers?: Timers;
  readonly reconnect?: ReconnectPolicy;
  readonly hooks?: ConnectionHooks;
  /** Sent in `rpc.hello`. */
  readonly client?: { readonly name: string; readonly version: string };
  /** Mints `request_id`s; tests inject a counter. */
  readonly newRequestId?: () => string;
  /**
   * Keep calls that carry a `request_id` across a lost connection (their promises stay pending
   * and are sent again after the next handshake) instead of rejecting them. Reads are always
   * rejected on loss: ask again. Default false.
   */
  readonly retainUnacked?: boolean;
  /** Send the outbox right after a handshake. Default true. */
  readonly autoFlush?: boolean;
}

interface RawPending {
  readonly resolve: (value: unknown) => void;
  readonly reject: (error: unknown) => void;
}

type Handler = (payload: unknown) => void;

const CODE_BY_WIRE: Readonly<Record<string, RpcError["code"]>> = {
  not_found: "not_found",
  locked: "locked",
  conflict: "conflict",
  invalid: "invalid",
  forbidden: "forbidden",
  unsupported: "unsupported",
  internal: "internal",
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Turns a JSON-RPC `error` member into the {@link RpcError} the typed client rejects with. */
export function errorFromWire(error: unknown): RpcError {
  const e = isRecord(error) ? error : {};
  const data = isRecord(e.data) ? e.data : undefined;
  const wireCode = typeof data?.code === "string" ? data.code : undefined;
  const code = (wireCode !== undefined ? CODE_BY_WIRE[wireCode] : undefined) ?? "internal";
  const message =
    typeof e.message === "string"
      ? e.message
      : typeof data?.message === "string"
        ? data.message
        : "";
  const path = typeof data?.path === "string" ? data.path : undefined;
  return new RpcError(code, message, path, data);
}

const LOST_MESSAGE = "the connection to the engine was lost";

/** JSON-RPC 2.0 over WebSocket to the engine: a {@link Transport} with a reconnect state machine. */
export class RpcConnection implements Transport {
  readonly outbox = new Outbox();
  readonly #options: RpcConnectionOptions;
  readonly #machine = new ConnectionMachine();
  readonly #timers: Timers;
  readonly #policy: ReconnectPolicy;
  readonly #newRequestId: () => string;
  readonly #handlers = new Map<string, Set<Handler>>();
  /** Event names the engine has been told about on the current socket. */
  readonly #remoteSubs = new Set<string>();
  readonly #rawPending = new Map<number, RawPending>();
  /** JSON-RPC id to outbox key, for the current socket. */
  readonly #byRpcId = new Map<number, string>();
  readonly #subWork = new Set<Promise<unknown>>();
  #socket: WebSocketLike | null = null;
  /** Identifies the current socket so late events of an older one are ignored. */
  #generation = 0;
  #nextId = 1;
  #attempt = 0;
  #retryTimer: unknown = null;
  #everReady = false;
  #hello: RpcServerHello | null = null;

  constructor(options: RpcConnectionOptions) {
    this.#options = options;
    this.#timers = options.timers ?? {
      setTimeout: (callback, ms) => globalThis.setTimeout(callback, ms),
      clearTimeout: (handle) => globalThis.clearTimeout(handle as number),
    };
    this.#policy = options.reconnect ?? backoffPolicy();
    this.#newRequestId = options.newRequestId ?? (() => uuidv7());
    this.#machine.onChange((state, previous) => {
      this.#options.hooks?.onStateChange?.(state, previous);
    });
  }

  get state(): ConnectionState {
    return this.#machine.state;
  }

  /** The last `rpc.hello` answer; `null` before the first handshake. */
  get hello(): RpcServerHello | null {
    return this.#hello;
  }

  /** Listens to state changes; returns the unsubscribe function. */
  onStateChange(listener: StateListener): () => void {
    return this.#machine.onChange(listener);
  }

  /** Starts connecting (from `idle` or `failed`). Calls made earlier wait for the handshake. */
  connect(): void {
    if (this.#machine.dispatch({ type: "connect" })) {
      void this.#open();
    }
  }

  /**
   * Skips the backoff and tries now, resolving the engine address again: for a supervisor that
   * reports a restarted engine. On a live socket it is dropped first.
   */
  reconnectNow(): void {
    this.#clearRetryTimer();
    const state = this.#machine.state;
    if (state === "reconnecting" || state === "failed") {
      this.#attempt = 0;
      this.connect();
    } else if (state === "ready" || state === "handshaking" || state === "connecting") {
      this.#lose({ code: 4000, reason: "reconnect requested" }, 0);
    }
  }

  /** Closes for good: pending calls are rejected, nothing is retried. */
  close(): void {
    this.#clearRetryTimer();
    if (!this.#machine.dispatch({ type: "close" })) {
      return;
    }
    this.#dropSocket(1000, "client closed");
    this.#rejectAllRaw();
    this.outbox.rejectWhere(new RpcError("unsupported", "the connection is closed"));
  }

  /** Resolves when the state is `ready`; rejects if it ends in `failed` or `closed` first. */
  whenReady(): Promise<void> {
    if (this.state === "ready") {
      return Promise.resolve();
    }
    if (this.state === "failed" || this.state === "closed") {
      return Promise.reject(new RpcError("unsupported", `the connection ${this.state}`));
    }
    return new Promise((resolve, reject) => {
      const off = this.#machine.onChange((state) => {
        if (state === "ready") {
          off();
          resolve();
        } else if (state === "failed" || state === "closed") {
          off();
          reject(new RpcError("unsupported", `the connection ${state}`));
        }
      });
    });
  }

  // ----- Transport ---------------------------------------------------------------------------

  request(method: string, params: unknown, meta?: RequestMeta): Promise<unknown> {
    return this.requestWithMeta(method, params, meta).then((r) => r.result);
  }

  /** Like {@link request}, also returning `result._meta` (`rev`, `committed_rev`, `tx_id`, ...). */
  requestWithMeta(
    method: string,
    params: unknown,
    meta?: RequestMeta,
  ): Promise<{ readonly result: unknown; readonly meta: Readonly<Record<string, unknown>> }> {
    const state = this.#machine.state;
    if (state === "closed" || state === "failed") {
      return Promise.reject(new RpcError("unsupported", `the connection ${state}`));
    }
    const requestId = needsRequestId(method) ? (meta?.request_id ?? this.#newRequestId()) : null;
    const envelope: Record<string, unknown> = { ...meta };
    if (requestId !== null) {
      envelope.request_id = requestId;
    }
    const base = isRecord(params) ? params : {};
    const sent: Record<string, unknown> =
      Object.keys(envelope).length === 0 ? { ...base } : { ...base, _meta: envelope };

    const waits = !this.#everReady || (requestId !== null && this.#options.retainUnacked === true);
    if (state === "reconnecting" && !waits) {
      return Promise.reject(new RpcError("unsupported", LOST_MESSAGE));
    }
    return new Promise((resolve, reject) => {
      const entry: OutboxEntry = {
        requestId,
        method,
        params: sent,
        sent: false,
        rpcId: null,
        resolve: (value) => resolve(value as never),
        reject,
      };
      const key = this.outbox.add(entry);
      if (this.#machine.state === "ready") {
        this.#send(key, entry);
      }
    });
  }

  subscribe(event: string, handler: (payload: unknown) => void): () => void {
    let set = this.#handlers.get(event);
    if (set === undefined) {
      set = new Set();
      this.#handlers.set(event, set);
    }
    set.add(handler);
    this.#trackSubscriptions();
    return () => {
      const current = this.#handlers.get(event);
      if (current === undefined) {
        return;
      }
      current.delete(handler);
      if (current.size === 0) {
        this.#handlers.delete(event);
        this.#trackSubscriptions();
      }
    };
  }

  /** Resolves once the engine has been told about every event subscribed so far. */
  async settled(): Promise<void> {
    while (this.#subWork.size > 0) {
      await Promise.allSettled([...this.#subWork]);
    }
  }

  /** Sends every entry the current socket has not been given yet, in call order. */
  flushOutbox(): void {
    if (this.#machine.state !== "ready") {
      return;
    }
    for (const [key, entry] of this.outbox.keyed()) {
      if (!entry.sent) {
        this.#send(key, entry);
      }
    }
  }

  // ----- sending -----------------------------------------------------------------------------

  #write(message: unknown): void {
    this.#socket?.send(JSON.stringify(message));
  }

  #send(key: string, entry: OutboxEntry): void {
    const id = this.#nextId++;
    entry.sent = true;
    entry.rpcId = id;
    this.#byRpcId.set(id, key);
    try {
      this.#write({ jsonrpc: "2.0", id, method: entry.method, params: entry.params });
    } catch (error) {
      // A socket that throws on `send` is going away: its close event settles the entry.
      entry.sent = false;
      this.#byRpcId.delete(id);
      this.#options.hooks?.onError?.(error);
    }
  }

  /** A request on the handshake path: sent whatever the state, never queued or retained. */
  #raw(method: string, params: unknown): Promise<unknown> {
    return new Promise((resolve, reject) => {
      const id = this.#nextId++;
      this.#rawPending.set(id, { resolve, reject });
      try {
        this.#write({ jsonrpc: "2.0", id, method, params });
      } catch (error) {
        this.#rawPending.delete(id);
        reject(error);
      }
    });
  }

  #rejectAllRaw(): void {
    const error = new RpcError("unsupported", LOST_MESSAGE);
    for (const pending of this.#rawPending.values()) {
      pending.reject(error);
    }
    this.#rawPending.clear();
  }

  // ----- receiving ---------------------------------------------------------------------------

  #receive(data: unknown): void {
    if (typeof data !== "string") {
      return;
    }
    let message: unknown;
    try {
      message = JSON.parse(data);
    } catch {
      return;
    }
    if (!isRecord(message)) {
      return;
    }
    const id = message.id;
    if (typeof id === "number" && ("result" in message || "error" in message)) {
      this.#settle(id, message);
    } else if (typeof message.method === "string" && id === undefined) {
      this.#dispatchEvent(message.method, message.params);
    }
  }

  #settle(id: number, message: Record<string, unknown>): void {
    const failure = "error" in message ? errorFromWire(message.error) : undefined;
    const raw = this.#rawPending.get(id);
    if (raw !== undefined) {
      this.#rawPending.delete(id);
      if (failure !== undefined) {
        raw.reject(failure);
      } else {
        raw.resolve(message.result);
      }
      return;
    }
    const key = this.#byRpcId.get(id);
    this.#byRpcId.delete(id);
    const entry = key === undefined ? undefined : this.outbox.get(key);
    if (key === undefined || entry === undefined || entry.rpcId !== id) {
      return;
    }
    this.outbox.delete(key);
    if (failure !== undefined) {
      entry.reject(failure);
      return;
    }
    entry.resolve(splitMeta(message.result));
  }

  #dispatchEvent(name: string, payload: unknown): void {
    for (const handler of [...(this.#handlers.get(name) ?? [])]) {
      try {
        handler(payload);
      } catch (error) {
        this.#options.hooks?.onError?.(error);
      }
    }
  }

  // ----- subscriptions -----------------------------------------------------------------------

  #trackSubscriptions(): void {
    if (this.#machine.state !== "ready") {
      return; // the handshake sends the full set
    }
    const work = this.#syncSubscriptions().catch((error: unknown) => {
      this.#options.hooks?.onError?.(error);
    });
    this.#subWork.add(work);
    void work.finally(() => this.#subWork.delete(work));
  }

  /** Brings the engine's subscription set in line with the handlers registered here. */
  async #syncSubscriptions(): Promise<void> {
    const wanted = new Set(this.#handlers.keys());
    const add = [...wanted].filter((name) => !this.#remoteSubs.has(name));
    const remove = [...this.#remoteSubs].filter((name) => !wanted.has(name));
    for (const name of add) {
      this.#remoteSubs.add(name);
    }
    for (const name of remove) {
      this.#remoteSubs.delete(name);
    }
    if (add.length > 0) {
      await this.#raw("rpc.subscribe", { events: add });
    }
    if (remove.length > 0) {
      await this.#raw("rpc.unsubscribe", { events: remove });
    }
  }

  // ----- connecting --------------------------------------------------------------------------

  async #open(): Promise<void> {
    let info: ConnectionInfo | null;
    try {
      info = await this.#options.resolve();
    } catch (error) {
      this.#options.hooks?.onError?.(error);
      info = null;
    }
    if (this.#machine.state !== "connecting") {
      return; // closed while resolving
    }
    if (info === null) {
      this.#lose({ code: 1006, reason: "no engine connection available" });
      return;
    }
    const generation = ++this.#generation;
    let socket: WebSocketLike;
    try {
      socket = (this.#options.createSocket ?? defaultSocketFactory)(info.wsUrl, [
        tokenSubprotocol(info.token),
      ]);
    } catch (error) {
      this.#options.hooks?.onError?.(error);
      this.#lose({ code: 1006, reason: "could not open the socket" });
      return;
    }
    this.#socket = socket;
    this.#remoteSubs.clear();
    socket.onopen = () => {
      if (generation === this.#generation && this.#machine.dispatch({ type: "socket_open" })) {
        void this.#handshake(generation);
      }
    };
    socket.onmessage = (event) => {
      if (generation === this.#generation) {
        this.#receive(event.data);
      }
    };
    socket.onerror = () => {
      // The close event follows and carries the loss.
    };
    socket.onclose = (event) => {
      if (generation === this.#generation) {
        this.#socket = null;
        this.#lose({ code: event.code, reason: event.reason });
      }
    };
  }

  async #handshake(generation: number): Promise<void> {
    const client = this.#options.client ?? { name: "cutweave-ui", version: "0.0.0" };
    try {
      const hello = (await this.#raw("rpc.hello", {
        client: client.name,
        version: client.version,
      })) as RpcServerHello;
      if (generation !== this.#generation) {
        return;
      }
      await this.#syncSubscriptions();
      if (generation !== this.#generation) {
        return;
      }
      this.#hello = hello;
      this.#attempt = 0;
      const reconnected = this.#everReady;
      this.#everReady = true;
      this.#machine.dispatch({ type: "handshake_ok" });
      if (hello.schema_hash !== SCHEMA_HASH) {
        this.#options.hooks?.onSchemaMismatch?.(hello);
      }
      if (this.#options.autoFlush !== false) {
        this.flushOutbox();
      }
      const context: ReconnectContext = { outbox: this.outbox, connection: this };
      const hooks = this.#options.hooks;
      try {
        await (reconnected ? hooks?.onReconnected : hooks?.onConnected)?.call(
          hooks,
          hello,
          context,
        );
      } catch (error) {
        hooks?.onError?.(error); // a failing hook does not cost the connection
      }
    } catch (error) {
      if (generation === this.#generation) {
        this.#options.hooks?.onError?.(error);
        // A failed handshake is a lost connection: the close event will not come on its own.
        this.#dropSocket(1011, "handshake failed");
        this.#lose({ code: 1011, reason: "handshake failed" });
      }
    }
  }

  // ----- losing and retrying -----------------------------------------------------------------

  /** The current socket ended (or an attempt failed): settle calls, tell the hooks, retry. */
  #lose(loss: SocketLoss, forcedDelayMs?: number): void {
    const wasReady = this.#machine.state === "ready";
    const event: ConnectionEvent = { type: "socket_lost", ...loss };
    if (this.#machine.state === "closed") {
      return;
    }
    this.#dropSocket(loss.code, loss.reason);
    if (!this.#machine.dispatch(event)) {
      return;
    }
    this.#rejectAllRaw();
    this.#byRpcId.clear();
    this.#remoteSubs.clear();
    this.#settleOnLoss();

    const delay = forcedDelayMs ?? this.#policy.nextDelayMs(this.#attempt, loss);
    this.#attempt += 1;
    if (wasReady) {
      this.#options.hooks?.onDisconnected?.({
        ...loss,
        willRetry: delay !== null,
        unacked: this.outbox.views(),
      });
    }
    if (delay === null) {
      this.#machine.dispatch({ type: "give_up" });
      this.outbox.rejectWhere(new RpcError("unsupported", "the engine is not reachable"));
      return;
    }
    this.#clearRetryTimer();
    this.#retryTimer = this.#timers.setTimeout(() => {
      this.#retryTimer = null;
      if (this.#machine.dispatch({ type: "retry" })) {
        void this.#open();
      }
    }, delay);
  }

  /**
   * Before the first handshake everything waits for it. Afterwards reads fail and calls with a
   * `request_id` stay only when `retainUnacked` is set; the survivors are marked unsent so the
   * next handshake sends them again under the same `request_id`.
   */
  #settleOnLoss(): void {
    const keepAll = !this.#everReady;
    const retain = this.#options.retainUnacked === true;
    const error = new RpcError("unsupported", LOST_MESSAGE);
    this.outbox.rejectWhere(error, (entry) => !(keepAll || (retain && entry.requestId !== null)));
    for (const [, entry] of this.outbox.keyed()) {
      entry.sent = false;
      entry.rpcId = null;
    }
  }

  #dropSocket(code: number, reason: string): void {
    const socket = this.#socket;
    this.#socket = null;
    this.#generation += 1; // late events of the dropped socket are ignored
    if (socket !== null) {
      socket.onopen = socket.onmessage = socket.onclose = socket.onerror = null;
      try {
        socket.close(code === 1005 || code === 1006 ? 1000 : code, reason.slice(0, 100));
      } catch {
        // already closed
      }
    }
  }

  #clearRetryTimer(): void {
    if (this.#retryTimer !== null) {
      this.#timers.clearTimeout(this.#retryTimer);
      this.#retryTimer = null;
    }
  }
}

/** Splits `result._meta` off a result (the typed outputs never contain it). */
function splitMeta(result: unknown): {
  readonly result: unknown;
  readonly meta: Readonly<Record<string, unknown>>;
} {
  if (isRecord(result) && isRecord(result._meta)) {
    const { _meta, ...rest } = result;
    return { result: rest, meta: _meta };
  }
  return { result, meta: {} };
}
