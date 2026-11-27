// SPDX-License-Identifier: GPL-3.0-or-later
// One pointer gesture as one engine gesture transaction (arch §5.2, WP-1M.06).
//
//   begin   `edit.begin_tx{kind:"gesture", label}` (after any earlier engine work of the panel)
//   update  `item.move` / `clip.trim` with `_meta.tx`, throttled: at most one per animation frame
//           and one in flight; only the newest position is sent, so N pointer moves cost at most
//           N updates, usually far fewer. The engine applies each live (the preview follows).
//   finish  commit: flush the newest position, then `edit.commit_tx` (one undo step); when the
//           engine refused the final position, `edit.cancel_tx` instead (the gesture reverts).
//           cancel (Esc): `edit.cancel_tx`, everything rolls back.
//
// A refusal a later position can fix (`invalid` overlap, `locked` track) leaves the clip at its
// last accepted position and the gesture open. Anything else (`conflict` with another actor's
// gesture, a lost transaction) aborts at once: rolled back and reported through `onAbort`.
import {
  type ClipTrim,
  type ItemMove,
  type ItemSetProps,
  type RpcClient,
  RpcError,
  type TimelineMoveMarker,
  type TxId,
} from "@cutweave/protocol";
import { activeDocument, framesOf } from "../shell/windows.ts";

export type GestureUpdate =
  | { readonly name: "item.move"; readonly input: ItemMove }
  | { readonly name: "clip.trim"; readonly input: ClipTrim }
  /** An inspector slider or scrubber drag (WP-1M.07). */
  | { readonly name: "item.set_props"; readonly input: ItemSetProps };

/** A marker dragged on the ruler (issue #222, `markerDrag.ts`). */
export interface MarkerMove {
  readonly name: "timeline.move_marker";
  readonly input: TimelineMoveMarker;
}

type Update = GestureUpdate | MarkerMove;

/** One position of the gesture: a command, or several that move together (a multi-selection). */
export type GestureStep = GestureUpdate | readonly GestureUpdate[];

export type GestureOutcome =
  /** Committed; `changed` is false when the clip ended where it started (no undo entry). */
  | { readonly kind: "committed"; readonly changed: boolean }
  /** Cancelled by the user (Esc): rolled back. */
  | { readonly kind: "cancelled" }
  /** The engine refused the final position (or the commit): rolled back. */
  | { readonly kind: "refused"; readonly error: RpcError }
  /**
   * Ended early on an error moving on could not fix: rolled back. `reported`: `onAbort` already
   * told the host (it happened mid-drag, before `finish`).
   */
  | { readonly kind: "aborted"; readonly error: RpcError; readonly reported: boolean };

/** Where updates wait for the next frame (`requestAnimationFrame` in the app). */
export interface FrameScheduler {
  request(callback: () => void): unknown;
  cancel(handle: unknown): void;
}

/**
 * The frames of the window the person is dragging in (a detached panel's own, WP-2H.15), or a
 * 16 ms timer where there are none (Node tests). The timeline passes its canvas's window instead.
 */
export const animationFrames: FrameScheduler = framesOf(() =>
  typeof globalThis.document === "undefined" ? null : activeDocument(),
);

export interface GestureOptions {
  readonly rpc: RpcClient;
  /** The undo entry's name ("Move clip"). */
  readonly label: string;
  readonly frames?: FrameScheduler;
  /** Engine work that must finish before this gesture opens (the previous gesture's commit). */
  readonly after?: Promise<unknown>;
  /** The gesture ended early; it is rolled back. Called at most once, never after `finish`. */
  readonly onAbort?: (error: RpcError) => void;
}

/** Refusals a later position can fix: the gesture stays open. */
const RECOVERABLE: ReadonlySet<string> = new Set(["invalid", "locked"]);

export function toRpcError(error: unknown): RpcError {
  return error instanceof RpcError ? error : new RpcError("internal", String(error));
}

const keyOf = (updates: readonly Update[]): string =>
  updates.map((u) => `${u.name}${JSON.stringify(u.input)}`).join("|");

const asList = (step: GestureStep | MarkerMove): readonly Update[] =>
  Array.isArray(step) ? (step as readonly Update[]) : [step as Update];

export class GestureSession {
  /** `item.move` / `clip.trim` requests sent so far. */
  updates = 0;

  readonly #rpc: RpcClient;
  readonly #frames: FrameScheduler;
  readonly #onAbort: ((error: RpcError) => void) | undefined;
  readonly #tx: Promise<TxId | null>;
  #pending: readonly Update[] | null = null;
  #lastSent: string | null = null;
  #lastError: RpcError | null = null;
  #inflight: Promise<void> | null = null;
  #frame: unknown = null;
  #framed = false;
  #closing = false;
  #aborted: RpcError | null = null;
  #reported = false;
  #outcome: Promise<GestureOutcome> | null = null;

  constructor(options: GestureOptions) {
    this.#rpc = options.rpc;
    this.#frames = options.frames ?? animationFrames;
    this.#onAbort = options.onAbort;
    const begin = async (): Promise<TxId> => {
      await options.after?.catch(() => undefined);
      const opened = await this.#rpc.call("edit.begin_tx", {
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
  update(update: GestureStep | MarkerMove): void {
    if (this.#closing) return;
    this.#pending = asList(update);
    this.#schedule();
  }

  /** Ends the gesture once (later calls return the same outcome). */
  finish(commit: boolean): Promise<GestureOutcome> {
    if (this.#outcome === null) {
      this.#closing = true;
      this.#unschedule();
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

  #unschedule(): void {
    if (this.#framed) this.#frames.cancel(this.#frame);
    this.#framed = false;
  }

  #pump(): void {
    if (this.#closing || this.#inflight !== null || this.#pending === null) return;
    const next = this.#pending;
    this.#pending = null;
    if (keyOf(next) === this.#lastSent) return;
    this.#inflight = this.#send(next).finally(() => {
      this.#inflight = null;
      if (this.#pending !== null) this.#schedule();
    });
  }

  async #send(updates: readonly Update[]): Promise<void> {
    const tx = await this.#tx;
    if (tx === null || this.#aborted !== null) return;
    this.#lastSent = keyOf(updates);
    try {
      for (const update of updates) {
        this.updates += 1;
        if (update.name === "item.move") await this.#rpc.call(update.name, update.input, { tx });
        else if (update.name === "clip.trim") {
          await this.#rpc.call(update.name, update.input, { tx });
        } else if (update.name === "timeline.move_marker") {
          await this.#rpc.call(update.name, update.input, { tx });
        } else await this.#rpc.call(update.name, update.input, { tx });
      }
      this.#lastError = null;
    } catch (error) {
      const failure = toRpcError(error);
      this.#lastError = failure;
      if (!RECOVERABLE.has(failure.code)) this.#abort(failure, true);
    }
  }

  #abort(error: RpcError, rollBack: boolean): void {
    if (this.#aborted !== null) return;
    this.#aborted = error;
    this.#pending = null;
    this.#unschedule();
    if (rollBack) void this.#cancelTx();
    if (this.#outcome === null) {
      this.#reported = true;
      this.#onAbort?.(error);
    }
  }

  async #cancelTx(): Promise<void> {
    const tx = await this.#tx;
    if (tx === null) return;
    // The transaction may already be gone (connection lost, idle expiry): nothing to undo then.
    await this.#rpc.call("edit.cancel_tx", { tx }).catch(() => undefined);
  }

  async #finish(commit: boolean): Promise<GestureOutcome> {
    if (this.#inflight !== null) await this.#inflight;
    const tx = await this.#tx;
    const pending = this.#pending;
    this.#pending = null;
    if (commit && pending !== null && keyOf(pending) !== this.#lastSent && tx !== null) {
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
    if (this.#lastError !== null) {
      await this.#cancelTx();
      return { kind: "refused", error: this.#lastError };
    }
    try {
      const done = await this.#rpc.call("edit.commit_tx", { tx });
      return { kind: "committed", changed: done.changed };
    } catch (error) {
      await this.#cancelTx();
      return { kind: "refused", error: toRpcError(error) };
    }
  }
}
