// SPDX-License-Identifier: GPL-3.0-or-later
// Keeps a `DocSink` current: subscribe, then snapshot, then apply `doc.patched` (rpc.md, "Resync:
// the revision-gap protocol"). A gap or a `doc.reset` triggers exactly one `project.snapshot`;
// events that arrive while it is in flight are held and replayed on top of it.

import type { ApplyResult, DocSink } from "./doc-mirror.ts";
import type { DocPatched } from "./generated/types.ts";
import type { RpcClient } from "./rpc.ts";

export interface DocSyncOptions {
  /** Failures of fire-and-forget resyncs (a lost connection, a throwing sink). */
  readonly onError?: (error: unknown) => void;
}

/** A snapshot is replayed against held events at most this often before giving up. */
const MAX_PASSES = 3;

export class DocSync {
  readonly #rpc: RpcClient;
  readonly #sink: DocSink;
  readonly #onError: (error: unknown) => void;
  #offs: (() => void)[] = [];
  /** Events received while a snapshot is in flight. */
  #held: DocPatched[] = [];
  #inflight: Promise<void> | null = null;
  #snapshots = 0;

  constructor(rpc: RpcClient, sink: DocSink, options: DocSyncOptions = {}) {
    this.#rpc = rpc;
    this.#sink = sink;
    this.#onError = options.onError ?? (() => {});
  }

  /** How many `project.snapshot` calls this sync has made. */
  get snapshotCount(): number {
    return this.#snapshots;
  }

  /** Subscribes to `doc.patched` and `doc.reset`, then fetches the first snapshot. */
  async start(): Promise<void> {
    if (this.#offs.length === 0) {
      this.#offs = [
        this.#rpc.on("doc.patched", (event) => {
          this.#onPatched(event);
        }),
        this.#rpc.on("doc.reset", () => {
          this.#background();
        }),
      ];
    }
    await this.resync();
  }

  /** Stops listening. */
  stop(): void {
    for (const off of this.#offs) {
      off();
    }
    this.#offs = [];
  }

  /**
   * Fetches `project.snapshot` and replaces the mirror. Concurrent calls share one fetch, so a
   * burst of gaps costs one snapshot. Call it after every (re)connect once the subscriptions
   * are back (`RpcConnection` restores them during the handshake).
   */
  resync(): Promise<void> {
    this.#inflight ??= this.#run().finally(() => {
      this.#inflight = null;
    });
    return this.#inflight;
  }

  #background(): void {
    this.resync().catch(this.#onError);
  }

  #onPatched(event: DocPatched): void {
    if (this.#inflight !== null) {
      this.#held.push(event);
      return;
    }
    if (this.#sink.applyEvent(event) === "gap") {
      this.#background();
    }
  }

  async #run(): Promise<void> {
    this.#held = [];
    for (let pass = 0; pass < MAX_PASSES; pass++) {
      // Subscribe first, then snapshot: nothing between the two can be missed.
      await this.#rpc.subscribed();
      this.#snapshots += 1;
      const snapshot = await this.#rpc.snapshot();
      this.#sink.resync(snapshot);
      const held = this.#held;
      this.#held = [];
      let result: ApplyResult = "stale";
      for (const event of held) {
        result = this.#sink.applyEvent(event);
        if (result === "gap") {
          break;
        }
      }
      if (result !== "gap") {
        return;
      }
    }
    throw new Error("the document mirror kept falling behind the engine");
  }
}
