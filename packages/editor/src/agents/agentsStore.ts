// SPDX-License-Identifier: GPL-3.0-or-later
// The agent connections open on this window's engine (WP-1J.05): Settings > Agents and MCP lists
// them, and a notice names an agent when it connects or disconnects. Fed by the engine runtime:
// `project.connections` after every handshake (a new baseline, not news), then the
// `project.connection_opened` and `project.connection_closed` events. While the page is open it
// also refreshes the last command of each connection.

import type { AgentConnection } from "@cutweave/protocol";
import { useStore } from "zustand";
import { createStore } from "zustand/vanilla";

export interface AgentsState {
  /** The open agent connections, oldest first. */
  readonly connections: readonly AgentConnection[];
  /** Bumped by `reset`: the list was read anew (a handshake), so the difference is not news. */
  readonly baseline: number;
  /** The engine's whole list (after a handshake). */
  reset(list: readonly AgentConnection[]): void;
  /** `project.connection_opened`. */
  opened(connection: AgentConnection): void;
  /** `project.connection_closed`. */
  closed(connection: AgentConnection): void;
  /** A fresh read of the list: only the last commands of connections already listed change. */
  refresh(list: readonly AgentConnection[]): void;
}

export function createAgentsStore() {
  return createStore<AgentsState>()((set) => ({
    connections: [],
    baseline: 0,
    reset: (list) => set((s) => ({ connections: [...list], baseline: s.baseline + 1 })),
    opened: (connection) =>
      set((s) =>
        s.connections.some((c) => c.id === connection.id)
          ? s
          : { connections: [...s.connections, connection] },
      ),
    closed: (connection) =>
      set((s) =>
        s.connections.some((c) => c.id === connection.id)
          ? { connections: s.connections.filter((c) => c.id !== connection.id) }
          : s,
      ),
    refresh: (list) =>
      set((s) => ({
        connections: s.connections.map((c) => {
          const fresh = list.find((f) => f.id === c.id);
          return fresh === undefined
            ? c
            : { ...c, last_command: fresh.last_command, last_command_at: fresh.last_command_at };
        }),
      })),
  }));
}

export const agentsStore = createAgentsStore();

export function useAgents<T>(selector: (state: AgentsState) => T): T {
  return useStore(agentsStore, selector);
}

// ---- notices ----------------------------------------------------------------------------------

export interface AgentNotice {
  readonly kind: "connected" | "disconnected";
  /** `agent:mcp`, `agent:cli`, `plugin:<id>`. */
  readonly actor: string;
}

export interface NoticeTimers {
  set(fn: () => void, ms: number): unknown;
  clear(handle: unknown): void;
}

const realTimers: NoticeTimers = {
  set: (fn, ms) => setTimeout(fn, ms),
  clear: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

/**
 * How long an agent may be without a connection before "disconnected" is said. Agents open a
 * connection per request on MCP's stateless revision, and a script runs `cutweave exec` once per
 * step: one notice per agent session, not one per request.
 */
export const DISCONNECT_GRACE_MS = 3000;

/**
 * One notice when an agent connects and one when it has been gone for a moment, per actor (an
 * agent with several connections is one agent). The connections already open at a handshake are
 * not news.
 */
export class AgentNotices {
  readonly #notify: (notice: AgentNotice) => void;
  readonly #graceMs: number;
  readonly #timers: NoticeTimers;
  readonly #open = new Map<string, number>();
  readonly #announced = new Set<string>();
  readonly #leaving = new Map<string, unknown>();

  constructor(options: {
    notify: (notice: AgentNotice) => void;
    graceMs?: number;
    timers?: NoticeTimers;
  }) {
    this.#notify = options.notify;
    this.#graceMs = options.graceMs ?? DISCONNECT_GRACE_MS;
    this.#timers = options.timers ?? realTimers;
  }

  /** Follows `store`; returns a function that stops. */
  watch(store: typeof agentsStore): () => void {
    this.baseline(store.getState().connections);
    return store.subscribe((state, previous) => {
      if (state.baseline !== previous.baseline) {
        this.baseline(state.connections);
        return;
      }
      const before = new Set(previous.connections.map((c) => c.id));
      const after = new Set(state.connections.map((c) => c.id));
      for (const c of state.connections) if (!before.has(c.id)) this.opened(c.actor);
      for (const c of previous.connections) if (!after.has(c.id)) this.closed(c.actor);
    });
  }

  /** The connections open now are known, not news. */
  baseline(connections: readonly AgentConnection[]): void {
    for (const handle of this.#leaving.values()) this.#timers.clear(handle);
    this.#leaving.clear();
    this.#open.clear();
    for (const c of connections) {
      this.#open.set(c.actor, (this.#open.get(c.actor) ?? 0) + 1);
      this.#announced.add(c.actor);
    }
    for (const actor of [...this.#announced]) {
      if (!this.#open.has(actor)) this.#announced.delete(actor);
    }
  }

  opened(actor: string): void {
    this.#open.set(actor, (this.#open.get(actor) ?? 0) + 1);
    const leaving = this.#leaving.get(actor);
    if (leaving !== undefined) {
      // Back within the grace period: still the same session.
      this.#timers.clear(leaving);
      this.#leaving.delete(actor);
      return;
    }
    if (this.#announced.has(actor)) return;
    this.#announced.add(actor);
    this.#notify({ kind: "connected", actor });
  }

  closed(actor: string): void {
    const left = Math.max(0, (this.#open.get(actor) ?? 0) - 1);
    if (left > 0) {
      this.#open.set(actor, left);
      return;
    }
    this.#open.delete(actor);
    if (!this.#announced.has(actor) || this.#leaving.has(actor)) return;
    const handle = this.#timers.set(() => {
      this.#leaving.delete(actor);
      this.#announced.delete(actor);
      this.#notify({ kind: "disconnected", actor });
    }, this.#graceMs);
    this.#leaving.set(actor, handle);
  }

  /** Stops the pending notices. */
  dispose(): void {
    for (const handle of this.#leaving.values()) this.#timers.clear(handle);
    this.#leaving.clear();
  }
}
