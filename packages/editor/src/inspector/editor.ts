// SPDX-License-Identifier: GPL-3.0-or-later
// Sends inspector edits to the engine (WP-1M.07). Framework-free, so the flows run against a
// real engine in Vitest; `InspectorPanel` binds it to React.
//
// - A slider or scrubber drag is one gesture transaction (`GestureSession`, arch §5.2): each
//   frame's newest value is sent as `item.set_props` with the transaction (the preview follows),
//   release commits one undo entry ("Change opacity"), Escape or dragging back to where it
//   started cancels (no entry).
// - A typed value (name, start, duration, a number typed into a scrubber) is one batch
//   transaction with the same label, so the undo entry reads the same.
// Several selected clips are edited inside the same transaction: one undo entry for all.

import type { RpcClient, TxId } from "@cutweave/protocol";
import type { StoreApi } from "zustand/vanilla";
import type { EngineStoreState } from "../engine/engineStore.ts";
import type { ToastStore } from "../state/toastStore.ts";
import { describeRefusal, type Translate } from "../timeline/errors.ts";
import {
  type FrameScheduler,
  GestureSession,
  type GestureStep,
  type GestureUpdate,
  toRpcError,
} from "../timeline/gesture.ts";
import type { FieldDef, SelectedClip } from "./fields.ts";

export interface InspectorTimers {
  setTimeout(callback: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

export interface InspectorEditorDeps {
  readonly engine: StoreApi<EngineStoreState>;
  readonly toasts: StoreApi<ToastStore>;
  readonly t: Translate;
  readonly frames?: FrameScheduler;
  readonly timers?: InspectorTimers;
}

/** A drag that ends where it began (Escape in a scrubber reports the start value) cancels after this long. */
export const RESTORED_CANCEL_MS = 200;

interface OpenDrag {
  readonly key: string;
  readonly field: FieldDef;
  readonly session: GestureSession;
  /** Per clip, the value the drag started from. */
  readonly start: readonly unknown[];
  timer: unknown;
}

const defaultTimers: InspectorTimers = {
  setTimeout: (callback, ms) => setTimeout(callback, ms),
  clearTimeout: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

export class InspectorEditor {
  readonly #deps: InspectorEditorDeps;
  #drag: OpenDrag | null = null;
  /** Engine work in order: the next edit starts after the previous one ended. */
  #queue: Promise<unknown> = Promise.resolve();

  constructor(deps: InspectorEditorDeps) {
    this.#deps = deps;
  }

  /** Resolves when every edit started so far has finished (tests). */
  settled(): Promise<void> {
    return this.#queue.then(() => undefined);
  }

  /** Whether a drag is open. */
  get dragging(): boolean {
    return this.#drag !== null;
  }

  #rpc(): RpcClient | null {
    const { rpc, blocked } = this.#deps.engine.getState();
    return blocked === null ? rpc : null;
  }

  #notify(field: FieldDef, error: unknown): void {
    const failure = toRpcError(error);
    this.#deps.toasts.getState().push({
      tone: "danger",
      title: this.#deps.t("inspector.failed"),
      message: `${this.#deps.t(field.label)}: ${describeRefusal(failure, this.#deps.t)}`,
    });
  }

  #steps(field: FieldDef, clips: readonly SelectedClip[], value: number | string): GestureUpdate[] {
    // The table types each control's value; the caller passes the matching kind.
    return clips.flatMap((clip) => {
      if (field.control === "text") return field.updates(clip, String(value));
      return field.updates(clip, Number(value));
    });
  }

  // ---- drags ----------------------------------------------------------------------------

  /**
   * A live value of a slider or scrubber drag. The first call opens the gesture; later calls
   * replace the pending value (at most one request per animation frame is sent).
   */
  live(field: FieldDef, clips: readonly SelectedClip[], value: number): void {
    const rpc = this.#rpc();
    if (rpc === null || clips.length === 0 || field.control === "text") return;
    const key = `${field.id}:${clips.map((c) => c.item.id).join(",")}`;
    let drag = this.#drag;
    if (drag !== null && drag.key !== key) {
      // A drag of another field started before this one ended: end it first.
      void this.commit(drag.field, [], 0);
      drag = null;
    }
    if (drag === null) {
      const label = this.#deps.t(field.history);
      const session: GestureSession = new GestureSession({
        rpc,
        label,
        after: this.#queue,
        ...(this.#deps.frames && { frames: this.#deps.frames }),
        onAbort: (error) => {
          if (this.#drag?.session === session) this.#drag = null;
          this.#notify(field, error);
        },
      });
      drag = {
        key,
        field,
        session,
        start: clips.map((c) => field.read(c.item)),
        timer: null,
      };
      this.#drag = drag;
    }
    this.#clearTimer(drag);
    drag.session.update(this.#steps(field, clips, value) as GestureStep);
    // Dragged back to the start (or Escape, which reports the start value): no entry wanted.
    const atStart =
      clips.length === drag.start.length && clips.every((_, i) => drag.start[i] === value);
    if (atStart) {
      const timers = this.#deps.timers ?? defaultTimers;
      const open = drag;
      open.timer = timers.setTimeout(() => {
        if (this.#drag === open) void this.cancel();
      }, RESTORED_CANCEL_MS);
    }
  }

  #clearTimer(drag: OpenDrag): void {
    if (drag.timer !== null) (this.#deps.timers ?? defaultTimers).clearTimeout(drag.timer);
    drag.timer = null;
  }

  /** The drag ended with a value (pointer release, key press): one undo entry. */
  async commit(field: FieldDef, clips: readonly SelectedClip[], value: number): Promise<void> {
    const drag = this.#drag;
    if (drag === null) {
      if (clips.length > 0) await this.apply(field, clips, value);
      return;
    }
    this.#drag = null;
    this.#clearTimer(drag);
    if (clips.length > 0) drag.session.update(this.#steps(drag.field, clips, value) as GestureStep);
    const done = drag.session.finish(true).then((outcome) => {
      if (outcome.kind === "refused" || (outcome.kind === "aborted" && !outcome.reported)) {
        this.#notify(drag.field, outcome.error);
      }
    });
    this.#queue = this.#queue.then(() => done);
    await done;
  }

  /** The drag was abandoned: everything rolls back, no undo entry. */
  async cancel(): Promise<void> {
    const drag = this.#drag;
    if (drag === null) return;
    this.#drag = null;
    this.#clearTimer(drag);
    const done = drag.session.finish(false).then(() => undefined);
    this.#queue = this.#queue.then(() => done);
    await done;
  }

  // ---- typed values ---------------------------------------------------------------------

  /** One typed value for every selected clip, as one batch (one undo entry). */
  apply(field: FieldDef, clips: readonly SelectedClip[], value: number | string): Promise<void> {
    const run = async (): Promise<void> => {
      const rpc = this.#rpc();
      if (rpc === null || clips.length === 0) return;
      const updates = this.#steps(field, clips, value);
      let tx: TxId | null = null;
      try {
        const opened = await rpc.call("edit.begin_tx", {
          kind: "batch",
          label: this.#deps.t(field.history),
        });
        tx = opened.tx;
        for (const update of updates) await callUpdate(rpc, update, tx);
        await rpc.call("edit.commit_tx", { tx });
      } catch (error) {
        if (tx !== null) await rpc.call("edit.cancel_tx", { tx }).catch(() => undefined);
        this.#notify(field, error);
      }
    };
    const work = this.#queue.then(run, run);
    this.#queue = work;
    return work;
  }

  /** The panel unmounts or the selection moves on: roll back an open drag. */
  dispose(): Promise<void> {
    return this.cancel();
  }
}

function callUpdate(rpc: RpcClient, update: GestureUpdate, tx: TxId): Promise<unknown> {
  const meta = { tx };
  switch (update.name) {
    case "item.move":
      return rpc.call(update.name, update.input, meta);
    case "clip.trim":
      return rpc.call(update.name, update.input, meta);
    case "item.set_props":
      return rpc.call(update.name, update.input, meta);
  }
}
