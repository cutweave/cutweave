// SPDX-License-Identifier: GPL-3.0-or-later
// Main's own JSON-RPC connection to the engine (docs/reference/rpc.md), used by the supervisor
// only: `rpc.hello` to learn which project is open, `project.recover` after a supervised
// restart, and the project events. The renderer has its own connection (@cutweave/protocol).
// Built on Node's WebSocket (no dependency). The UI token travels as the `cutweave.token.<t>`
// subprotocol, never in the URL.

import { randomBytes } from "node:crypto";

/** A JSON-RPC error answer: `code` is the JSON-RPC code, `dataCode` the `CmdError` code. */
export class EngineRpcError extends Error {
  readonly code: number;
  readonly dataCode: string | undefined;
  constructor(code: number, message: string, dataCode?: string) {
    super(message);
    this.name = "EngineRpcError";
    this.code = code;
    this.dataCode = dataCode;
  }
}

export type NotificationListener = (method: string, params: unknown) => void;

/** What the supervisor needs from an engine connection (tests pass a fake). */
export interface EngineRpc {
  call(method: string, params?: Record<string, unknown>, timeoutMs?: number): Promise<unknown>;
  onNotification(listener: NotificationListener): void;
  /** Called once when the socket closes (engine stopped or crashed: no close frame). */
  onClose(listener: () => void): void;
  close(): void;
}

/** The slice of the WHATWG WebSocket the client uses. */
export interface WebSocketLike {
  readonly readyState: number;
  send(data: string): void;
  close(code?: number, reason?: string): void;
  addEventListener(type: "open" | "close" | "error", listener: () => void): void;
  addEventListener(type: "message", listener: (event: { data: unknown }) => void): void;
}

export type WebSocketFactory = (url: string, protocols: string[]) => WebSocketLike;

const OPEN = 1;
const DEFAULT_CALL_TIMEOUT_MS = 10_000;

/** A UUIDv7 (time-ordered) for `_meta.request_id` (ADR-0015). */
export function uuidv7(now: number = Date.now()): string {
  const b = randomBytes(16);
  let ms = BigInt(Math.max(0, Math.floor(now)));
  for (let i = 5; i >= 0; i--) {
    b[i] = Number(ms & 0xffn);
    ms >>= 8n;
  }
  b[6] = ((b[6] ?? 0) & 0x0f) | 0x70;
  b[8] = ((b[8] ?? 0) & 0x3f) | 0x80;
  const hex = b.toString("hex");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

const defaultFactory: WebSocketFactory = (url, protocols) =>
  new WebSocket(url, protocols) as unknown as WebSocketLike;

export interface ConnectOptions {
  timeoutMs?: number;
  /** Defaults to Node's global WebSocket. */
  webSocket?: WebSocketFactory;
}

/** Open `ws://127.0.0.1:<port>/rpc` with the UI token; rejects on 401/403, close or timeout. */
export function connectEngineRpc(
  port: number,
  token: string,
  options: ConnectOptions = {},
): Promise<EngineRpc> {
  const factory = options.webSocket ?? defaultFactory;
  const ws = factory(`ws://127.0.0.1:${port}/rpc`, [`cutweave.token.${token}`]);
  return new Promise((resolve, reject) => {
    let settled = false;
    const timer = setTimeout(() => {
      if (settled) return;
      settled = true;
      ws.close();
      reject(new Error("timed out connecting to the engine"));
    }, options.timeoutMs ?? 5_000);
    ws.addEventListener("open", () => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(new RpcClient(ws));
    });
    const fail = () => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      reject(new Error("the engine refused or closed the connection"));
    };
    ws.addEventListener("error", fail);
    ws.addEventListener("close", fail);
  });
}

interface Pending {
  resolve(value: unknown): void;
  reject(err: Error): void;
  timer: ReturnType<typeof setTimeout>;
}

class RpcClient implements EngineRpc {
  readonly #ws: WebSocketLike;
  readonly #pending = new Map<number, Pending>();
  readonly #notify = new Set<NotificationListener>();
  readonly #closeListeners = new Set<() => void>();
  #nextId = 1;
  #closed = false;

  constructor(ws: WebSocketLike) {
    this.#ws = ws;
    ws.addEventListener("message", (event) => this.#onMessage(event.data));
    ws.addEventListener("close", () => this.#onClosed());
    ws.addEventListener("error", () => this.#onClosed());
  }

  call(method: string, params?: Record<string, unknown>, timeoutMs?: number): Promise<unknown> {
    if (this.#closed || this.#ws.readyState !== OPEN) {
      return Promise.reject(new Error("the engine connection is closed"));
    }
    const id = this.#nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pending.delete(id);
        reject(new Error(`${method} timed out`));
      }, timeoutMs ?? DEFAULT_CALL_TIMEOUT_MS);
      this.#pending.set(id, { resolve, reject, timer });
      this.#ws.send(JSON.stringify({ jsonrpc: "2.0", id, method, ...(params ? { params } : {}) }));
    });
  }

  onNotification(listener: NotificationListener): void {
    this.#notify.add(listener);
  }

  onClose(listener: () => void): void {
    if (this.#closed) listener();
    else this.#closeListeners.add(listener);
  }

  close(): void {
    if (this.#closed) return;
    try {
      this.#ws.close(1000, "supervisor");
    } catch {
      // Already closing.
    }
    this.#onClosed();
  }

  #onClosed(): void {
    if (this.#closed) return;
    this.#closed = true;
    for (const [id, p] of this.#pending) {
      clearTimeout(p.timer);
      p.reject(new Error("the engine connection closed"));
      this.#pending.delete(id);
    }
    for (const listener of this.#closeListeners) listener();
    this.#closeListeners.clear();
  }

  #onMessage(data: unknown): void {
    if (typeof data !== "string") return;
    let msg: unknown;
    try {
      msg = JSON.parse(data);
    } catch {
      return;
    }
    for (const m of Array.isArray(msg) ? msg : [msg]) this.#dispatch(m);
  }

  #dispatch(m: unknown): void {
    if (typeof m !== "object" || m === null) return;
    const r = m as {
      id?: unknown;
      method?: unknown;
      params?: unknown;
      result?: unknown;
      error?: unknown;
    };
    if (typeof r.method === "string" && r.id === undefined) {
      for (const listener of this.#notify) listener(r.method, r.params);
      return;
    }
    if (typeof r.id !== "number") return;
    const pending = this.#pending.get(r.id);
    if (pending === undefined) return;
    this.#pending.delete(r.id);
    clearTimeout(pending.timer);
    if (r.error !== undefined) {
      const e = (r.error ?? {}) as { code?: unknown; message?: unknown; data?: { code?: unknown } };
      pending.reject(
        new EngineRpcError(
          typeof e.code === "number" ? e.code : 0,
          typeof e.message === "string" ? e.message : "engine error",
          typeof e.data?.code === "string" ? e.data.code : undefined,
        ),
      );
    } else {
      pending.resolve(r.result);
    }
  }
}
