// SPDX-License-Identifier: GPL-3.0-or-later
// The outbox: every call that has not been answered yet, keyed by its `request_id`. The engine
// deduplicates a repeated `request_id` (10-minute window, rpc.md), so an entry can be sent again
// after a reconnect without applying twice. WP-1L.03 decides what to re-send and when; this
// module only keeps the entries and settles their promises.

import type { RpcError } from "./rpc.ts";

/** What hooks and tests may see of an entry (never the promise callbacks). */
export interface OutboxEntryView {
  /** The idempotency key; `null` for reads, which carry none. */
  readonly requestId: string | null;
  readonly method: string;
  /** The params as sent, `_meta` included. */
  readonly params: Readonly<Record<string, unknown>>;
  /** Whether the current socket has been given the request. */
  readonly sent: boolean;
}

export interface OutboxEntry extends OutboxEntryView {
  sent: boolean;
  /** The JSON-RPC id on the socket the entry was last sent over. */
  rpcId: number | null;
  resolve(value: unknown): void;
  reject(error: unknown): void;
}

/** Calls waiting for a response (or for a connection), in call order. */
export class Outbox {
  /** Keyed by `request_id`, or by a private key for reads. */
  readonly #entries = new Map<string, OutboxEntry>();
  #readSeq = 0;

  get size(): number {
    return this.#entries.size;
  }

  /** Adds an entry; returns its key. */
  add(entry: OutboxEntry): string {
    const key = entry.requestId ?? `read:${this.#readSeq++}`;
    this.#entries.set(key, entry);
    return key;
  }

  get(requestId: string): OutboxEntry | undefined {
    return this.#entries.get(requestId);
  }

  delete(key: string): void {
    this.#entries.delete(key);
  }

  /** The entries in call order, as hooks see them. */
  views(): readonly OutboxEntryView[] {
    return [...this.#entries.values()].map(view);
  }

  /** Entries with their keys, for the connection's own bookkeeping. */
  keyed(): readonly (readonly [string, OutboxEntry])[] {
    return [...this.#entries];
  }

  /** Rejects and removes every entry matching `where` (all by default). */
  rejectWhere(error: RpcError, where: (entry: OutboxEntry) => boolean = () => true): void {
    for (const [key, entry] of [...this.#entries]) {
      if (where(entry)) {
        this.#entries.delete(key);
        entry.reject(error);
      }
    }
  }
}

function view(entry: OutboxEntry): OutboxEntryView {
  return {
    requestId: entry.requestId,
    method: entry.method,
    params: entry.params,
    sent: entry.sent,
  };
}
