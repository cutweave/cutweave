// SPDX-License-Identifier: GPL-3.0-or-later
// The connection state machine, kept pure and apart from the socket so it can be tested with
// fake timers and extended by WP-1L.03 (engine restart, banner) without touching the wire code.

/**
 * - `idle`: created, `connect()` not called yet.
 * - `connecting`: opening the socket.
 * - `handshaking`: socket open, `rpc.hello` and the re-subscription are in flight.
 * - `ready`: calls and events flow.
 * - `reconnecting`: the socket was lost; waiting for the next attempt.
 * - `failed`: the retry policy gave up.
 * - `closed`: `close()` was called; final.
 */
export type ConnectionState =
  | "idle"
  | "connecting"
  | "handshaking"
  | "ready"
  | "reconnecting"
  | "failed"
  | "closed";

/** Why a socket ended: the WebSocket close code and reason (1006 when the peer just vanished). */
export interface SocketLoss {
  readonly code: number;
  readonly reason: string;
}

export type ConnectionEvent =
  /** Start, or force an attempt now (from `idle`, `failed` or `reconnecting`). */
  | { readonly type: "connect" }
  | { readonly type: "socket_open" }
  | { readonly type: "handshake_ok" }
  /** The socket closed, or an attempt to open one failed. */
  | ({ readonly type: "socket_lost" } & SocketLoss)
  /** The backoff timer fired. */
  | { readonly type: "retry" }
  | { readonly type: "give_up" }
  | { readonly type: "close" };

const TRANSITIONS: Readonly<
  Record<ConnectionState, Partial<Record<ConnectionEvent["type"], ConnectionState>>>
> = {
  idle: { connect: "connecting", close: "closed" },
  connecting: { socket_open: "handshaking", socket_lost: "reconnecting", close: "closed" },
  handshaking: { handshake_ok: "ready", socket_lost: "reconnecting", close: "closed" },
  ready: { socket_lost: "reconnecting", close: "closed" },
  reconnecting: { retry: "connecting", connect: "connecting", give_up: "failed", close: "closed" },
  failed: { connect: "connecting", close: "closed" },
  closed: {},
};

/** The state `event` leads to from `state`, or `undefined` when it is not allowed there. */
export function nextState(
  state: ConnectionState,
  event: ConnectionEvent["type"],
): ConnectionState | undefined {
  return TRANSITIONS[state][event];
}

export type StateListener = (state: ConnectionState, previous: ConnectionState) => void;

/** Holds the state and applies events; illegal events are ignored (`dispatch` returns false). */
export class ConnectionMachine {
  #state: ConnectionState = "idle";
  readonly #listeners = new Set<StateListener>();

  get state(): ConnectionState {
    return this.#state;
  }

  dispatch(event: ConnectionEvent): boolean {
    const next = nextState(this.#state, event.type);
    if (next === undefined) {
      return false;
    }
    const previous = this.#state;
    this.#state = next;
    for (const listener of [...this.#listeners]) {
      listener(next, previous);
    }
    return true;
  }

  /** Listens to state changes; returns the unsubscribe function. */
  onChange(listener: StateListener): () => void {
    this.#listeners.add(listener);
    return () => {
      this.#listeners.delete(listener);
    };
  }
}

/** When to try again after a loss: a delay in ms, or `null` to give up. */
export interface ReconnectPolicy {
  /** `attempt` counts from 0 for the first retry after a loss and resets once `ready`. */
  nextDelayMs(attempt: number, lost: SocketLoss): number | null;
}

/** Close code the server uses when a client must resync (rpc.md): retry at once. */
export const CLOSE_RESYNC_REQUIRED = 4001;

/**
 * Exponential backoff from `baseMs` up to `maxMs`, `maxAttempts` tries, and an immediate first
 * retry after `4001 resync_required`. WP-1L.03 replaces it with the supervisor-aware policy.
 */
export function backoffPolicy(
  options: { baseMs?: number; maxMs?: number; maxAttempts?: number } = {},
): ReconnectPolicy {
  const baseMs = options.baseMs ?? 250;
  const maxMs = options.maxMs ?? 4000;
  const maxAttempts = options.maxAttempts ?? 20;
  return {
    nextDelayMs(attempt, lost) {
      if (attempt >= maxAttempts) {
        return null;
      }
      if (attempt === 0 && lost.code === CLOSE_RESYNC_REQUIRED) {
        return 0;
      }
      return Math.min(maxMs, baseMs * 2 ** attempt);
    },
  };
}
