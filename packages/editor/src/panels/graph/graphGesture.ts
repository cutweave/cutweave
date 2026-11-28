// SPDX-License-Identifier: GPL-3.0-or-later
// One drag in the graph editor as one engine gesture transaction (WP-2C.07, arch §5.2, ADR-0023).
//
//   begin   `edit.begin_tx{kind:"gesture", label}`, after the panel's earlier engine work
//   update  a step: one or more `keyframe.*` commands. The first carries `_meta.replace`, which
//           rolls the transaction's earlier changes back, so every step starts from the keys as
//           they were when the drag began and addresses them by their stored times. At most one
//           step per animation frame and one in flight; only the newest position is sent.
//   finish  commit: the newest step goes out, then `edit.commit_tx` (one undo step). If the
//           engine refused that step (a key landing on another, a value out of range), the last
//           step it accepted is applied again and committed; with none, `edit.cancel_tx`.
//           cancel (Escape): `edit.cancel_tx`, everything rolls back.
//
// A refusal a later position can fix leaves the gesture open. Anything else (another actor's
// gesture holds the clip, the transaction is gone) ends it: rolled back, told once (`onAbort`).
import { type RequestMeta, type RpcClient, RpcError, type TxId } from "@cutweave/protocol";
import { animationFrames, type FrameScheduler, toRpcError } from "../../timeline/gesture.ts";

/** Sends one command of a step inside the gesture's transaction. */
export type StepSend = (name: string, args: unknown) => Promise<unknown>;

/** One position of the drag. */
export interface GraphStep {
  /** Two steps with the same key are the same position: the second is not sent. */
  readonly key: string;
  /** Sends the commands; resolves with what the host wants back when the step is applied. */
  run(send: StepSend): Promise<unknown>;
}

export type GraphOutcome =
  /** Committed as one undo step; `output` is what the applied step's `run` resolved with. */
  | { readonly kind: "committed"; readonly output: unknown }
  /** Nothing was applied (the pointer never left where it started, or Escape). */
  | { readonly kind: "cancelled" }
  /** The engine refused every position, or the commit: rolled back. */
  | { readonly kind: "refused"; readonly error: RpcError }
  | { readonly kind: "aborted"; readonly error: RpcError; readonly reported: boolean };

export interface GraphGestureOptions {
  readonly rpc: RpcClient;
  /** The undo step's name ("Move keyframe"). */
  readonly label: string;
  readonly frames?: FrameScheduler;
  /** Engine work that must finish first (the previous edit of the panel). */
  readonly after?: Promise<unknown>;
  /** A step was applied; `output` is what its `run` resolved with. */
  readonly onApplied?: (output: unknown) => void;
  /** The gesture ended early; it is rolled back. Called at most once, never after `finish`. */
  readonly onAbort?: (error: RpcError) => void;
}

const RECOVERABLE: ReadonlySet<string> = new Set(["invalid", "locked", "not_found"]);

type Call = (name: string, args: unknown, meta?: RequestMeta) => Promise<unknown>;

export class GraphGesture {
  /** Steps sent so far (tests read the traffic). */
  steps = 0;

  readonly #call: Call;
  readonly #frames: FrameScheduler;
  readonly #options: GraphGestureOptions;
  readonly #tx: Promise<TxId | null>;
  #pending: GraphStep | null = null;
  #lastSent: string | null = null;
  /** The newest step the engine applied whole. */
  #good: { step: GraphStep; output: unknown } | null = null;
  /** What is in the transaction now is exactly `#good` (false after a refused step). */
  #clean = true;
  #lastError: RpcError | null = null;
  #inflight: Promise<void> | null = null;
  #frame: unknown = null;
  #framed = false;
  #closing = false;
  #aborted: RpcError | null = null;
  #reported = false;
  #outcome: Promise<GraphOutcome> | null = null;

  constructor(options: GraphGestureOptions) {
    this.#options = options;
    this.#call = options.rpc.call.bind(options.rpc) as unknown as Call;
    this.#frames = options.frames ?? animationFrames;
    const begin = async (): Promise<TxId> => {
      await options.after?.catch(() => undefined);
      const opened = await options.rpc.call("edit.begin_tx", {
        kind: "gesture",
        label: options.label,
      });
      return opened.tx;
    };
    this.#tx = begin().catch((error: unknown) => {
      this.#abort(toRpcError(error), false);
      return null;
    });
  }

  /** The newest position; sent on the next frame unless a newer one replaces it first. */
  update(step: GraphStep): void {
    if (this.#closing) return;
    this.#pending = step;
    this.#schedule();
  }

  /** Ends the gesture once (later calls return the same outcome). */
  finish(commit: boolean): Promise<GraphOutcome> {
    if (this.#outcome === null) {
      this.#closing = true;
      if (this.#framed) this.#frames.cancel(this.#frame);
      this.#framed = false;
      this.#outcome = this.#finish(commit);
    }
    return this.#outcome;
  }

  #schedule(): void {
    if (this.#framed || this.#inflight !== null || this.#closing) return;
    this.#framed = true;
    this.#frame = this.#frames.request(() => {
      this.#framed = false;
      this.#pump();
    });
  }

  #pump(): void {
    if (this.#closing || this.#inflight !== null || this.#pending === null) return;
    const next = this.#pending;
    this.#pending = null;
    if (next.key === this.#lastSent) return;
    this.#inflight = this.#send(next).finally(() => {
      this.#inflight = null;
      if (this.#pending !== null) this.#schedule();
    });
  }

  async #send(step: GraphStep): Promise<void> {
    const tx = await this.#tx;
    if (tx === null || this.#aborted !== null) return;
    this.#lastSent = step.key;
    this.steps += 1;
    let first = true;
    const send: StepSend = (name, args) => {
      const meta: RequestMeta = first ? { tx, replace: true } : { tx };
      first = false;
      return this.#call(name, args, meta);
    };
    try {
      const output = await step.run(send);
      this.#good = { step, output };
      this.#clean = true;
      this.#lastError = null;
      this.#options.onApplied?.(output);
    } catch (error) {
      const failure = toRpcError(error);
      this.#lastError = failure;
      // Part of the step may be applied: what the transaction holds is no longer a whole step.
      this.#clean = false;
      if (!RECOVERABLE.has(failure.code)) this.#abort(failure, true);
    }
  }

  #abort(error: RpcError, rollBack: boolean): void {
    if (this.#aborted !== null) return;
    this.#aborted = error;
    this.#pending = null;
    if (this.#framed) this.#frames.cancel(this.#frame);
    this.#framed = false;
    if (rollBack) void this.#cancelTx();
    if (this.#outcome === null) {
      this.#reported = true;
      this.#options.onAbort?.(error);
    }
  }

  async #cancelTx(): Promise<void> {
    const tx = await this.#tx;
    if (tx === null) return;
    await this.#call("edit.cancel_tx", { tx }).catch(() => undefined);
  }

  async #finish(commit: boolean): Promise<GraphOutcome> {
    if (this.#inflight !== null) await this.#inflight;
    const tx = await this.#tx;
    const pending = this.#pending;
    this.#pending = null;
    if (commit && pending !== null && pending.key !== this.#lastSent && tx !== null) {
      await this.#send(pending);
    }
    if (this.#aborted !== null || tx === null) {
      return {
        kind: "aborted",
        error: this.#aborted ?? new RpcError("internal", "the gesture did not open"),
        reported: this.#reported,
      };
    }
    if (!commit) {
      await this.#cancelTx();
      return { kind: "cancelled" };
    }
    const good = this.#good;
    if (good === null) {
      await this.#cancelTx();
      return this.#lastError === null
        ? { kind: "cancelled" }
        : { kind: "refused", error: this.#lastError };
    }
    if (!this.#clean) {
      // The newest position was refused: put back the last one the engine took, whole.
      this.#lastSent = null;
      await this.#send(good.step);
      if (this.#aborted !== null || !this.#clean) {
        await this.#cancelTx();
        return {
          kind: "refused",
          error: this.#lastError ?? new RpcError("internal", "the edit could not be applied"),
        };
      }
    }
    try {
      await this.#call("edit.commit_tx", { tx });
      return { kind: "committed", output: this.#good?.output };
    } catch (error) {
      await this.#cancelTx();
      return { kind: "refused", error: toRpcError(error) };
    }
  }
}
