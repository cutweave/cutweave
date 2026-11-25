// SPDX-License-Identifier: GPL-3.0-or-later
// Keeps `historyStore` current (WP-2H.11): reads `edit.history` page by page and again whenever
// the engine says the stacks moved (`edit.history_changed`) or the document was replaced
// (`doc.reset`). Reads coalesce: a burst of events while one read runs costs one more read.
// Also the jump itself: `goToStep` runs the `edit.undo` / `edit.redo` steps a row asks for.

import { type HistoryEntry, type RpcClient, RpcError } from "@cutweave/protocol";
import type { StoreApi } from "zustand/vanilla";
import { runCommand } from "../engine/hooks.ts";
import { chunkSteps, type HistoryRow, planJump } from "./historyModel.ts";
import { type HistoryStore, historyStore } from "./historyStore.ts";

/** Entries per page (the engine's pages go up to 500). */
const PAGE = 200;
/** Pages read at most: the engine keeps a bounded history, this keeps a read bounded too. */
const MAX_PAGES = 10;

export interface HistoryFeedOptions {
  readonly rpc: RpcClient;
  readonly store?: StoreApi<HistoryStore>;
  readonly onError?: (error: unknown) => void;
}

export class HistoryFeed {
  readonly #rpc: RpcClient;
  readonly #store: StoreApi<HistoryStore>;
  readonly #onError: (error: unknown) => void;
  #offs: (() => void)[] = [];
  #reading = false;
  #again = false;
  #stopped = false;

  constructor(options: HistoryFeedOptions) {
    this.#rpc = options.rpc;
    this.#store = options.store ?? historyStore;
    this.#onError = options.onError ?? (() => undefined);
  }

  /** Subscribes and reads the first page(s). Call once. */
  start(): void {
    this.#offs.push(
      this.#rpc.on("edit.history_changed", () => void this.refresh()),
      this.#rpc.on("doc.reset", () => void this.refresh()),
    );
    void this.refresh();
  }

  stop(): void {
    this.#stopped = true;
    for (const off of this.#offs) off();
    this.#offs = [];
  }

  /** Reads the whole list again (several pages when it is long). */
  async refresh(): Promise<void> {
    if (this.#reading) {
      this.#again = true;
      return;
    }
    this.#reading = true;
    try {
      do {
        this.#again = false;
        const entries: HistoryEntry[] = [];
        let cursor: string | undefined;
        for (let page = 0; page < MAX_PAGES; page += 1) {
          const out = await this.#rpc.call("edit.history", {
            limit: PAGE,
            ...(cursor !== undefined && { cursor }),
          });
          entries.push(...out.entries);
          if (out.next_cursor === null || out.next_cursor === undefined) break;
          cursor = out.next_cursor;
        }
        if (this.#stopped) return;
        this.#store.getState().setEntries(entries);
      } while (this.#again && !this.#stopped);
    } catch (error) {
      this.#onError(error);
    } finally {
      this.#reading = false;
    }
  }
}

/** What a jump did, for the panel's message. */
export interface JumpResult {
  /** Steps applied. */
  readonly done: number;
  /** Steps asked for. */
  readonly wanted: number;
  /** The engine's message when the jump stopped early, else `null`. */
  readonly stopped: string | null;
}

/**
 * Goes back (or forward) to the state right after the row `txId`: `*` undos or redos, one run of
 * at most 100 steps per call, each step its own history record. A refused step stops the jump
 * where it is; the message names the later change and who made it.
 */
export async function goToStep(
  rows: readonly HistoryRow[],
  txId: string,
  store: StoreApi<HistoryStore> = historyStore,
): Promise<JumpResult> {
  const plan = planJump(rows, txId);
  if (plan.kind === "none") return { done: 0, wanted: 0, stopped: null };
  const command = plan.kind === "undo" ? "edit.undo" : "edit.redo";
  const state = store.getState();
  state.setBusy(true);
  state.setNotice(null);
  let done = 0;
  let stopped: string | null = null;
  try {
    for (const steps of chunkSteps(plan.steps)) {
      const out = await runCommand(command, { actor: "*", steps });
      done += out.done ?? 1;
      if (out.stopped !== undefined && out.stopped !== null) {
        stopped = out.stopped;
        break;
      }
    }
  } catch (error) {
    stopped = error instanceof RpcError || error instanceof Error ? error.message : String(error);
  } finally {
    store.getState().setBusy(false);
  }
  if (stopped !== null) store.getState().setNotice(stopped);
  return { done, wanted: plan.steps, stopped };
}
