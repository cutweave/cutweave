// SPDX-License-Identifier: GPL-3.0-or-later
import type { CmdErrorCode, RpcMap } from "./generated/commands.ts";
import type { EventMap } from "./generated/events.ts";
import type { Project } from "./generated/types.ts";

/** What the engine sends back for a failed call (`CmdError`, arch §5.1). */
export class RpcError extends Error {
  readonly code: CmdErrorCode;
  /** JSON Pointer into the params for `invalid`, when the engine names one. */
  readonly path: string | undefined;
  /** The rest of the wire `CmdError` (`current_rev`, `retry_after_ms`, ...), when there is any. */
  readonly data: Readonly<Record<string, unknown>> | undefined;

  constructor(
    code: CmdErrorCode,
    message: string,
    path?: string,
    data?: Readonly<Record<string, unknown>>,
  ) {
    super(message);
    this.name = "RpcError";
    this.code = code;
    this.path = path;
    this.data = data;
  }
}

/** The `params._meta` envelope fields a caller may set (rpc.md "Envelope"). */
export interface RequestMeta {
  /** Idempotency key (UUIDv7). The connection mints one for mutating calls when absent. */
  readonly request_id?: string;
  /** Fail with `conflict` unless the document is at this revision. */
  readonly expected_rev?: number;
  /** Run inside this open transaction of the connection. */
  readonly tx?: string;
}

/** Result of `project.snapshot`: the live document and its revisions (rpc.md). */
export interface ProjectSnapshot {
  readonly rev: number;
  readonly committed_rev: number;
  readonly project: Project;
}

/**
 * The wire under an {@link RpcClient}: JSON-RPC over WebSocket in production (WP-1I.02 / 1M.02),
 * {@link MockTransport} in tests. A failed request rejects with an {@link RpcError}.
 */
export interface Transport {
  /** Sends one request and resolves with its `result` (without `_meta`). */
  request(method: string, params: unknown, meta?: RequestMeta): Promise<unknown>;
  /** Subscribes to one event by wire name; returns the unsubscribe function. */
  subscribe(event: string, handler: (payload: unknown) => void): () => void;
  /**
   * Resolves once the engine delivers every event subscribed so far. A network transport needs a
   * round trip for that; the resync protocol (rpc.md) subscribes first and only then reads the
   * snapshot. Absent on transports where `subscribe` takes effect at once.
   */
  settled?(): Promise<void>;
}

/** Typed facade over a {@link Transport}: names, inputs, outputs and events come from the schema. */
export class RpcClient {
  readonly #transport: Transport;

  constructor(transport: Transport) {
    this.#transport = transport;
  }

  /** Runs a command or query. Unknown names and wrong inputs are compile errors. */
  call<K extends keyof RpcMap>(
    name: K,
    input: RpcMap[K]["input"],
    meta?: RequestMeta,
  ): Promise<RpcMap[K]["output"]> {
    return this.#transport.request(name, input, meta) as Promise<RpcMap[K]["output"]>;
  }

  /** Reads the live document and its revisions (`project.snapshot`, a reserved method). */
  snapshot(): Promise<ProjectSnapshot> {
    return this.#transport.request("project.snapshot", {}) as Promise<ProjectSnapshot>;
  }

  /** Resolves once every subscription made so far is in effect on the engine. */
  async subscribed(): Promise<void> {
    await this.#transport.settled?.();
  }

  /**
   * Listens to an event the generated {@link EventMap} does not know yet (`playback.state`,
   * `view.changed`: WP-1I.05 / 1C.04 export their payloads later). The payload is `unknown`:
   * narrow it before use.
   */
  onUntyped(name: string, handler: (payload: unknown) => void): () => void {
    return this.#transport.subscribe(name, handler);
  }

  /** Listens to an event; returns the unsubscribe function. */
  on<E extends keyof EventMap>(name: E, handler: (payload: EventMap[E]) => void): () => void {
    return this.#transport.subscribe(name, (payload) => handler(payload as EventMap[E]));
  }
}

/** A request recorded by {@link MockTransport}. */
export interface RecordedRequest {
  readonly method: string;
  readonly params: unknown;
  readonly meta?: RequestMeta;
}

type Handler = (params: unknown) => unknown;

/** In-memory transport for tests: canned handlers per method, and `emit` to push events. */
export class MockTransport implements Transport {
  readonly requests: RecordedRequest[] = [];
  /** Set it to observe or delay what {@link RpcClient.subscribed} waits for. */
  settled?: () => Promise<void>;
  readonly #handlers = new Map<string, Handler>();
  readonly #listeners = new Map<string, Set<(payload: unknown) => void>>();

  /** Answers `method` with `handler(params)` (a thrown {@link RpcError} rejects the call). */
  handle(method: string, handler: Handler): this {
    this.#handlers.set(method, handler);
    return this;
  }

  request(method: string, params: unknown, meta?: RequestMeta): Promise<unknown> {
    this.requests.push(meta === undefined ? { method, params } : { method, params, meta });
    const handler = this.#handlers.get(method);
    if (handler === undefined) {
      return Promise.reject(new RpcError("not_found", `unknown method \`${method}\``));
    }
    try {
      return Promise.resolve(handler(params));
    } catch (error) {
      return Promise.reject(error);
    }
  }

  subscribe(event: string, handler: (payload: unknown) => void): () => void {
    let set = this.#listeners.get(event);
    if (set === undefined) {
      set = new Set();
      this.#listeners.set(event, set);
    }
    set.add(handler);
    return () => {
      set.delete(handler);
    };
  }

  /** Delivers `payload` to every subscriber of `event`. */
  emit(event: string, payload: unknown): void {
    for (const handler of [...(this.#listeners.get(event) ?? [])]) {
      handler(payload);
    }
  }
}
