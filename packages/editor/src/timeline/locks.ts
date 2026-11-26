// SPDX-License-Identifier: GPL-3.0-or-later
// Who is editing which clip right now (WP-2A.12, arch §5.2): the soft locks of open gestures and
// proposals, read from `edit.locks` and kept current by the `tx.opened` / `tx.locked` /
// `tx.closed` events (and read again after `doc.reset`). The timeline marks the clips another
// actor holds "in use by <actor>"; the person's own gestures (actor `user`) are left out. The
// editor never retries a refused edit on its own: the marks only tell the person who and why.
import type { LockList, RpcClient, TxActivity, TxEnded } from "@cutweave/protocol";
import { createStore, type StoreApi } from "zustand/vanilla";

/** The person's own actor: their gestures lock clips for others, never for themselves. */
const PERSON = "user";

/** One open transaction's locked items. */
export interface HeldItems {
  readonly actor: string;
  readonly kind: string;
  readonly items: readonly string[];
}

export interface LocksState {
  /** Open transactions by id. */
  readonly txs: ReadonlyMap<string, HeldItems>;
  /** Item id to the actor holding it, for every item another actor's transaction locks. */
  readonly byItem: ReadonlyMap<string, string>;
  /** A new baseline (`edit.locks`). */
  reset(list: LockList): void;
  /** `tx.opened` / `tx.locked`: the transaction's items so far. */
  held(activity: TxActivity): void;
  /** `tx.closed`: its locks are released. */
  released(ended: TxEnded): void;
  clear(): void;
}

function byItemOf(txs: ReadonlyMap<string, HeldItems>): ReadonlyMap<string, string> {
  const out = new Map<string, string>();
  for (const held of txs.values()) {
    if (held.actor === PERSON) continue;
    for (const item of held.items) out.set(item, held.actor);
  }
  return out;
}

export function createLocksStore(): StoreApi<LocksState> {
  const next = (txs: Map<string, HeldItems>) => ({ txs, byItem: byItemOf(txs) });
  return createStore<LocksState>()((set) => ({
    txs: new Map(),
    byItem: new Map(),
    reset(list) {
      const txs = new Map<string, HeldItems>();
      for (const t of list.transactions) {
        txs.set(t.tx, { actor: t.actor, kind: t.kind, items: t.items });
      }
      set(next(txs));
    },
    held(activity) {
      set((s) => {
        const txs = new Map(s.txs);
        txs.set(activity.tx_id, {
          actor: activity.actor,
          kind: activity.kind,
          items: activity.items,
        });
        return next(txs);
      });
    },
    released(ended) {
      set((s) => {
        if (!s.txs.has(ended.tx_id)) return s;
        const txs = new Map(s.txs);
        txs.delete(ended.tx_id);
        return next(txs);
      });
    },
    clear() {
      set(next(new Map()));
    },
  }));
}

/** The locks the timeline shows. */
export const locksStore = createLocksStore();

/**
 * Keeps a {@link LocksState} current from one engine connection. Reads coalesce: a refresh asked
 * for while one runs costs one more read.
 */
export class LocksFeed {
  readonly #rpc: RpcClient;
  readonly #store: StoreApi<LocksState>;
  readonly #onError: (error: unknown) => void;
  #offs: (() => void)[] = [];
  #reading = false;
  #again = false;
  #stopped = false;

  constructor(options: {
    readonly rpc: RpcClient;
    readonly store?: StoreApi<LocksState>;
    readonly onError?: (error: unknown) => void;
  }) {
    this.#rpc = options.rpc;
    this.#store = options.store ?? locksStore;
    this.#onError = options.onError ?? (() => undefined);
  }

  /** Subscribes and reads the baseline. Call once. */
  start(): void {
    const store = this.#store.getState();
    this.#offs.push(
      this.#rpc.on("tx.opened", (a) => store.held(a)),
      this.#rpc.on("tx.locked", (a) => store.held(a)),
      this.#rpc.on("tx.closed", (e) => store.released(e)),
      this.#rpc.on("doc.reset", () => void this.refresh()),
    );
    void this.refresh();
  }

  stop(): void {
    this.#stopped = true;
    for (const off of this.#offs) off();
    this.#offs = [];
    this.#store.getState().clear();
  }

  /** Reads `edit.locks` again (after a reconnect: events may have been missed). */
  async refresh(): Promise<void> {
    if (this.#reading) {
      this.#again = true;
      return;
    }
    this.#reading = true;
    try {
      do {
        this.#again = false;
        const list = await this.#rpc.call("edit.locks", {});
        if (this.#stopped) return;
        this.#store.getState().reset(list);
      } while (this.#again && !this.#stopped);
    } catch (error) {
      this.#onError(error);
    } finally {
      this.#reading = false;
    }
  }
}
