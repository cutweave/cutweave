// SPDX-License-Identifier: GPL-3.0-or-later
import type { CmdErrorCode, RpcMap } from "./generated/commands.ts";
import type { EventMap } from "./generated/events.ts";

/** What the engine sends back for a failed call (`CmdError`, arch §5.1). */
export class RpcError extends Error {
  readonly code: CmdErrorCode;
  /** JSON Pointer into the params for `invalid`, when the engine names one. */
  readonly path: string | undefined;

  constructor(code: CmdErrorCode, message: string, path?: string) {
    super(message);
    this.name = "RpcError";
    this.code = code;
    this.path = path;
  }
}

/**
 * The wire under an {@link RpcClient}: JSON-RPC over WebSocket in production (WP-1I.02 / 1M.02),
 * {@link MockTransport} in tests. A failed request rejects with an {@link RpcError}.
 */
export interface Transport {
  /** Sends one request and resolves with its `result`. */
  request(method: string, params: unknown): Promise<unknown>;
  /** Subscribes to one event by wire name; returns the unsubscribe function. */
  subscribe(event: string, handler: (payload: unknown) => void): () => void;
}

/** Typed facade over a {@link Transport}: names, inputs, outputs and events come from the schema. */
export class RpcClient {
  readonly #transport: Transport;

  constructor(transport: Transport) {
    this.#transport = transport;
  }

  /** Runs a command or query. Unknown names and wrong inputs are compile errors. */
  call<K extends keyof RpcMap>(name: K, input: RpcMap[K]["input"]): Promise<RpcMap[K]["output"]> {
    return this.#transport.request(name, input) as Promise<RpcMap[K]["output"]>;
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
}

type Handler = (params: unknown) => unknown;

/** In-memory transport for tests: canned handlers per method, and `emit` to push events. */
export class MockTransport implements Transport {
  readonly requests: RecordedRequest[] = [];
  readonly #handlers = new Map<string, Handler>();
  readonly #listeners = new Map<string, Set<(payload: unknown) => void>>();

  /** Answers `method` with `handler(params)` (a thrown {@link RpcError} rejects the call). */
  handle(method: string, handler: Handler): this {
    this.#handlers.set(method, handler);
    return this;
  }

  request(method: string, params: unknown): Promise<unknown> {
    this.requests.push({ method, params });
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
