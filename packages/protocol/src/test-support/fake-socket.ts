// SPDX-License-Identifier: GPL-3.0-or-later
// Test support: an in-memory WebSocket and a scripted "engine" behind it, so the connection's
// handshake, correlation and reconnect behaviour can be tested without a network.

import type { SocketFactory, Timers, WebSocketLike } from "../connection.ts";
import { SCHEMA_HASH } from "../generated/schema-hash.ts";

export interface SentMessage {
  readonly id?: number;
  readonly method: string;
  readonly params: Record<string, unknown>;
}

/** The socket as the connection sees it, plus the controls of the fake server behind it. */
export class FakeSocket implements WebSocketLike {
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { readonly data: unknown }) => void) | null = null;
  onclose: ((event: { readonly code: number; readonly reason: string }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  readonly sent: SentMessage[] = [];
  closedByClient: { code?: number; reason?: string } | null = null;

  constructor(
    readonly url: string,
    readonly protocols: string[],
    /** Called for every message the client sends; answer with `reply`. */
    private readonly onSend: (socket: FakeSocket, message: SentMessage) => void,
  ) {}

  send(data: string): void {
    const message = JSON.parse(data) as SentMessage;
    this.sent.push(message);
    this.onSend(this, message);
  }

  close(code?: number, reason?: string): void {
    this.closedByClient = {
      ...(code !== undefined && { code }),
      ...(reason !== undefined && { reason }),
    };
  }

  /** Server side: the upgrade succeeded. */
  open(): void {
    this.onopen?.({});
  }

  /** Server side: a response to request `id`. */
  reply(id: number, result: unknown): void {
    this.onmessage?.({ data: JSON.stringify({ jsonrpc: "2.0", id, result }) });
  }

  replyError(id: number, code: number, data: Record<string, unknown>, message = "failed"): void {
    this.onmessage?.({
      data: JSON.stringify({ jsonrpc: "2.0", id, error: { code, message, data } }),
    });
  }

  /** Server side: an event notification. */
  emit(method: string, params: unknown): void {
    this.onmessage?.({ data: JSON.stringify({ jsonrpc: "2.0", method, params }) });
  }

  /** Server side: the socket ends. */
  serverClose(code = 1006, reason = ""): void {
    this.onclose?.({ code, reason });
  }

  methods(): string[] {
    return this.sent.map((m) => m.method);
  }
}

export const HELLO = {
  engine_version: "0.0.0",
  schema_hash: SCHEMA_HASH,
  rev: 0,
  committed_rev: 0,
  actor: "user",
  project: { state: "no_project", path: null, name: "Untitled", rev: 0 },
};

/**
 * A fake engine: answers `rpc.hello` and `rpc.subscribe` itself and hands every other request to
 * `handler` (default: no answer, so tests answer by hand with `socket.reply`).
 */
export class FakeEngine {
  readonly sockets: FakeSocket[] = [];
  hello: Record<string, unknown> = HELLO;
  /** Opens sockets on their own as soon as they are created. */
  autoOpen = true;
  handler: ((socket: FakeSocket, message: SentMessage) => unknown) | null = null;

  readonly factory: SocketFactory = (url, protocols) => {
    const socket = new FakeSocket(url, protocols, (s, m) => this.#onSend(s, m));
    this.sockets.push(socket);
    if (this.autoOpen) {
      queueMicrotask(() => socket.open());
    }
    return socket;
  };

  get last(): FakeSocket {
    const socket = this.sockets.at(-1);
    if (socket === undefined) {
      throw new Error("no socket was created");
    }
    return socket;
  }

  #onSend(socket: FakeSocket, message: SentMessage): void {
    if (message.id === undefined) {
      return;
    }
    if (message.method === "rpc.hello") {
      socket.reply(message.id, this.hello);
    } else if (message.method === "rpc.subscribe" || message.method === "rpc.unsubscribe") {
      socket.reply(message.id, { events: message.params.events ?? [] });
    } else if (this.handler !== null) {
      const result = this.handler(socket, message);
      if (result !== undefined) {
        socket.reply(message.id, result);
      }
    }
  }
}

/** Timers a test advances by hand. */
export class ManualTimers implements Timers {
  readonly #tasks = new Map<number, { at: number; run: () => void }>();
  #now = 0;
  #next = 1;

  setTimeout(callback: () => void, ms: number): unknown {
    const id = this.#next++;
    this.#tasks.set(id, { at: this.#now + ms, run: callback });
    return id;
  }

  clearTimeout(handle: unknown): void {
    this.#tasks.delete(handle as number);
  }

  get pending(): number {
    return this.#tasks.size;
  }

  /** The delay of the next due task, or `undefined`. */
  get nextDelay(): number | undefined {
    const at = Math.min(...[...this.#tasks.values()].map((t) => t.at));
    return Number.isFinite(at) ? at - this.#now : undefined;
  }

  /** Runs every task due within `ms` milliseconds, in time order. */
  advance(ms: number): void {
    const end = this.#now + ms;
    for (;;) {
      const due = [...this.#tasks.entries()]
        .filter(([, t]) => t.at <= end)
        .sort((a, b) => a[1].at - b[1].at)[0];
      if (due === undefined) {
        break;
      }
      this.#tasks.delete(due[0]);
      this.#now = Math.max(this.#now, due[1].at);
      due[1].run();
    }
    this.#now = end;
  }
}

/** Lets promise continuations run (the connection awaits between handshake steps). */
export async function flushPromises(rounds = 10): Promise<void> {
  for (let i = 0; i < rounds; i++) {
    await Promise.resolve();
  }
}
