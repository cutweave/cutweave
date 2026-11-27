// SPDX-License-Identifier: GPL-3.0-or-later
// The engine side of the interaction machine's gestures (WP-2A.10, arch §5.2, timeline spec
// §3.9 and INV-301/302). The machine (WP-2A.08) says when a gesture opens, what its update is now
// (at most one per animation frame) and how it ends; this carries that out:
//
//   begin_tx   `edit.begin_tx{kind:"gesture", label}`, after the panel's earlier engine work
//   update_tx  the command with `_meta: {tx, replace: true}`: the engine replaces the gesture's
//              earlier update with it. One request in flight; a newer update waiting replaces an
//              older one (latest wins). Each answer goes back as `engine.result`.
//   commit_tx  the last update goes out, then `edit.commit_tx` (one undo step). Nothing applied
//              (every update refused): `edit.cancel_tx`, and the refusal is told.
//   cancel_tx  `edit.cancel_tx`; nothing to do when the engine closed it (`gesture.cancelled`,
//              the connection lost it).
//
// The machine decides what a refusal means (`invalid`, `locked` and `conflict{item_in_gesture}`
// keep the gesture; anything else ends it); this only reports and tells the person once.
import {
  isTransactionLost,
  type RequestMeta,
  type RpcClient,
  RpcError,
  type TxId,
} from "@cutweave/protocol";
import type { CancelReason, GestureEdit, MachineCommand, MachineEvent } from "@cutweave/timeline";
import type { RttProbe } from "./rttProbe.ts";

export function toRpcError(error: unknown): RpcError {
  return error instanceof RpcError ? error : new RpcError("internal", String(error));
}

/** Refusals a later position can fix: the engine kept the previous update (INV-302). */
export function isRecoverable(error: RpcError): boolean {
  return (
    error.code === "invalid" ||
    error.code === "locked" ||
    (error.code === "conflict" && error.data?.reason === "item_in_gesture")
  );
}

/** Engine work in order: a gesture opens after the earlier edits of the panel finished. */
export class EngineQueue {
  #tail: Promise<unknown> = Promise.resolve();

  /** Runs `work` after everything queued so far (whether that succeeded or not). */
  run<T>(work: () => Promise<T>): Promise<T> {
    const next = this.#tail.then(work, work);
    this.#tail = next.catch(() => undefined);
    return next;
  }

  /** Resolves once everything queued so far has finished. */
  settled(): Promise<void> {
    return this.#tail.then(() => undefined);
  }
}

export interface GestureTxDeps {
  rpc(): RpcClient | null;
  readonly queue: EngineQueue;
  /** The undo step's name ("Move clip"). */
  label(edit: GestureEdit): string;
  /** An answer for the machine (`engine.result`, `engine.cancelled`). */
  answer(event: MachineEvent): void;
  /** The gesture ended without its edit: tell the person (a toast). */
  refused(edit: GestureEdit, error: RpcError): void;
  /**
   * The gesture is about to commit: more commands for its transaction (the height of a track it
   * created, WP-2A.13). `output` is what its last applied update returned. Never fails the edit.
   */
  beforeCommit?(edit: GestureEdit, output: unknown, tx: TxId): Promise<void>;
  /** The gesture committed; `output` is what its last applied update returned. */
  committed(edit: GestureEdit, output: unknown): void;
  readonly probe?: RttProbe;
}

interface Open {
  readonly gesture: number;
  readonly edit: GestureEdit;
  tx: Promise<TxId | null>;
  txId: TxId | null;
  pending: MachineCommand | null;
  inflight: Promise<void> | null;
  /** Updates the engine applied. */
  applied: number;
  lastOutput: unknown;
  /** The latest update's refusal (cleared by a later success). */
  refusal: RpcError | null;
  /** The engine ended the gesture (a refusal no position fixes); why. */
  ended: RpcError | null;
  /** The engine already closed the transaction: nothing to cancel. */
  closedByEngine: boolean;
  closing: boolean;
}

type Call = (name: string, args: unknown, meta?: RequestMeta) => Promise<unknown>;

/** The gesture transactions of one timeline panel. */
export class GestureTransactions {
  readonly #deps: GestureTxDeps;
  readonly #open = new Map<number, Open>();
  readonly #byTx = new Map<string, number>();
  /** Updates sent (tests read the engine traffic). */
  updates = 0;

  constructor(deps: GestureTxDeps) {
    this.#deps = deps;
  }

  /** Whether a gesture is still open (not yet committed or cancelled). */
  get busy(): boolean {
    return this.#open.size > 0;
  }

  begin(gesture: number, edit: GestureEdit): void {
    const open: Open = {
      gesture,
      edit,
      tx: Promise.resolve(null),
      txId: null,
      pending: null,
      inflight: null,
      applied: 0,
      lastOutput: undefined,
      refusal: null,
      ended: null,
      closedByEngine: false,
      closing: false,
    };
    this.#open.set(gesture, open);
    this.#deps.probe?.reset();
    const rpc = this.#deps.rpc();
    open.tx = this.#deps.queue
      .run(async () => {
        if (rpc === null) throw new RpcError("unsupported", "the engine is not connected");
        const opened = await rpc.call("edit.begin_tx", {
          kind: "gesture",
          label: this.#deps.label(edit),
        });
        return opened.tx;
      })
      .then(
        (tx) => {
          open.txId = tx;
          this.#byTx.set(tx, gesture);
          this.#pump(open);
          return tx;
        },
        (error: unknown) => {
          // The gesture never opened: the machine goes back to idle; the person is told why.
          open.ended = toRpcError(error);
          open.closedByEngine = true;
          if (!open.closing) {
            this.#deps.answer({ type: "engine.cancelled", gesture, reason: open.ended.code });
          }
          return null;
        },
      );
  }

  update(gesture: number, command: MachineCommand): void {
    const open = this.#open.get(gesture);
    if (open === undefined || open.closing || open.ended !== null) return;
    open.pending = command;
    this.#pump(open);
  }

  commit(gesture: number): void {
    const open = this.#open.get(gesture);
    if (open === undefined || open.closing) return;
    open.closing = true;
    void this.#deps.queue.run(() => this.#commit(open));
  }

  cancel(gesture: number, reason: CancelReason): void {
    const open = this.#open.get(gesture);
    if (open === undefined || open.closing) return;
    open.closing = true;
    open.pending = null;
    void this.#deps.queue.run(() => this.#cancel(open, reason));
  }

  /** The engine's `gesture.cancelled` (idle timeout): the machine is told, nothing to roll back. */
  engineCancelled(tx: string, reason: string): void {
    const gesture = this.#byTx.get(tx);
    const open = gesture === undefined ? undefined : this.#open.get(gesture);
    if (open === undefined) return;
    open.closedByEngine = true;
    open.ended ??= new RpcError(
      "not_found",
      `transaction ${tx} was cancelled: ${reason}`,
      undefined,
      { kind: "transaction" },
    );
    open.pending = null;
    this.#deps.answer({ type: "engine.cancelled", gesture: open.gesture, reason });
  }

  /** Sends the waiting update when nothing is in flight. */
  #pump(open: Open): void {
    const rpc = this.#deps.rpc();
    const tx = open.txId;
    const command = open.pending;
    if (open.inflight !== null || command === null || tx === null || open.ended !== null) return;
    if (rpc === null) return;
    open.pending = null;
    const token = this.#deps.probe?.start();
    this.updates += 1;
    const call = rpc.call.bind(rpc) as unknown as Call;
    open.inflight = call(command.name, command.args, { tx, replace: true })
      .then(
        (output) => {
          open.applied += 1;
          open.lastOutput = output;
          open.refusal = null;
          if (token !== undefined) this.#deps.probe?.answered(token);
          const clamped = (output as { clamped?: unknown } | null)?.clamped === true;
          this.#answer(open, { type: "engine.result", gesture: open.gesture, ok: true, clamped });
        },
        (error: unknown) => {
          const refusal = toRpcError(error);
          open.refusal = refusal;
          if (!isRecoverable(refusal)) {
            open.ended = refusal;
            open.pending = null;
            if (isTransactionLost(refusal)) open.closedByEngine = true;
          }
          const reason = refusal.data?.reason;
          const holder = refusal.data?.holder;
          this.#answer(open, {
            type: "engine.result",
            gesture: open.gesture,
            ok: false,
            code: refusal.code,
            ...(typeof reason === "string" && { reason }),
            ...(typeof holder === "string" && { holder }),
          });
        },
      )
      .finally(() => {
        open.inflight = null;
        this.#pump(open);
      });
  }

  /** Answers go to the machine only while it still holds the gesture open. */
  #answer(open: Open, event: MachineEvent): void {
    if (!open.closing) this.#deps.answer(event);
  }

  /** Waits for the update in flight and sends the one still waiting. */
  async #drain(open: Open): Promise<void> {
    for (;;) {
      if (open.inflight !== null) {
        await open.inflight;
        continue;
      }
      if (open.pending === null || open.ended !== null || open.txId === null) return;
      this.#pump(open);
    }
  }

  async #commit(open: Open): Promise<void> {
    const rpc = this.#deps.rpc();
    try {
      const tx = await open.tx;
      await this.#drain(open);
      if (tx === null || rpc === null) {
        if (open.ended !== null) this.#deps.refused(open.edit, open.ended);
        return;
      }
      if (open.ended !== null) {
        // The engine ended it mid-drag (the machine was told and went idle first).
        if (!open.closedByEngine) await rpc.call("edit.cancel_tx", { tx }).catch(() => undefined);
        this.#deps.refused(open.edit, open.ended);
        return;
      }
      if (open.applied === 0) {
        // Every position was refused: nothing to keep, and the person learns why.
        await rpc.call("edit.cancel_tx", { tx }).catch(() => undefined);
        if (open.refusal !== null) this.#deps.refused(open.edit, open.refusal);
        return;
      }
      try {
        await this.#deps.beforeCommit?.(open.edit, open.lastOutput, tx).catch(() => undefined);
        await rpc.call("edit.commit_tx", { tx });
      } catch (error) {
        await rpc.call("edit.cancel_tx", { tx }).catch(() => undefined);
        this.#deps.refused(open.edit, toRpcError(error));
        return;
      }
      this.#deps.committed(open.edit, open.lastOutput);
    } finally {
      this.#close(open);
    }
  }

  async #cancel(open: Open, reason: CancelReason): Promise<void> {
    const rpc = this.#deps.rpc();
    try {
      const tx = await open.tx;
      if (open.inflight !== null) await open.inflight;
      if (tx !== null && rpc !== null && !open.closedByEngine) {
        // `reason: "engine"`: the engine closed it already in the usual case; a cancel that
        // finds nothing open is harmless and frees the clips if it did not.
        await rpc.call("edit.cancel_tx", { tx }).catch(() => undefined);
      }
      if (reason === "engine" && open.ended !== null) this.#deps.refused(open.edit, open.ended);
    } finally {
      this.#close(open);
    }
  }

  #close(open: Open): void {
    this.#open.delete(open.gesture);
    if (open.txId !== null) this.#byTx.delete(open.txId);
  }
}
