// SPDX-License-Identifier: GPL-3.0-or-later
// Sends the preview handles' edits to the engine (WP-2H.06). Framework-free, like the
// inspector's editor: a handle drag is one gesture transaction (`GestureSession`, arch §5.2) of
// `item.set_props` calls, so the preview follows live and release is one undo entry; a key nudge
// is one batch transaction. Animated properties are keyed at the playhead (`at`; the engine
// ignores it for static ones), so dragging an animated clip adds a keyframe there.
import type {
  CropPatch,
  FlipPatch,
  ItemId,
  ItemProps,
  RpcClient,
  TransformPatch,
  TxId,
} from "@cutweave/protocol";
import type { StoreApi } from "zustand/vanilla";
import type { EngineStoreState } from "../../engine/engineStore.ts";
import type { ToastStore } from "../../state/toastStore.ts";
import { describeRefusal, type Translate } from "../../timeline/errors.ts";
import {
  type FrameScheduler,
  GestureSession,
  type GestureStep,
  type GestureUpdate,
  toRpcError,
} from "../../timeline/gesture.ts";

export interface OverlayEditorDeps {
  readonly engine: StoreApi<EngineStoreState>;
  readonly toasts: StoreApi<ToastStore>;
  readonly t: Translate;
  readonly frames?: FrameScheduler;
}

/** The properties a handle changes. */
export interface PropsPatch {
  readonly transform?: Partial<TransformPatch>;
  readonly crop?: Partial<CropPatch>;
  readonly flip?: Partial<FlipPatch>;
}

/** One `item.set_props` of a handle edit; `at` keys animated properties at the playhead. */
export function setPropsStep(item: ItemId, patch: PropsPatch, at: number): GestureUpdate {
  const props: ItemProps = {
    name: null,
    enabled: null,
    opacity: null,
    audio_gain: null,
    audio_pan: null,
    transform:
      patch.transform === undefined
        ? null
        : { position: null, scale: null, rotation: null, anchor: null, ...patch.transform },
    crop:
      patch.crop === undefined
        ? null
        : { left: null, top: null, right: null, bottom: null, ...patch.crop },
    flip: patch.flip === undefined ? null : { horizontal: null, vertical: null, ...patch.flip },
    blend_mode: null,
  };
  return { name: "item.set_props", input: { item, props, at } };
}

export class OverlayEditor {
  readonly #deps: OverlayEditorDeps;
  #drag: GestureSession | null = null;
  /** Engine work in order: the next edit starts after the previous one ended. */
  #queue: Promise<unknown> = Promise.resolve();

  constructor(deps: OverlayEditorDeps) {
    this.#deps = deps;
  }

  /** Resolves when every edit started so far has finished (tests). */
  settled(): Promise<void> {
    return this.#queue.then(() => undefined);
  }

  get dragging(): boolean {
    return this.#drag !== null;
  }

  #rpc(): RpcClient | null {
    const { rpc, blocked } = this.#deps.engine.getState();
    return blocked === null ? rpc : null;
  }

  #notify(error: unknown): void {
    this.#deps.toasts.getState().push({
      tone: "danger",
      title: this.#deps.t("preview.overlay.failed"),
      message: describeRefusal(toRpcError(error), this.#deps.t),
    });
  }

  /** The newest position of a drag; the first call opens the gesture named `label`. */
  live(label: string, step: GestureStep): void {
    const rpc = this.#rpc();
    if (rpc === null) return;
    let drag = this.#drag;
    if (drag === null) {
      const session: GestureSession = new GestureSession({
        rpc,
        label,
        after: this.#queue,
        ...(this.#deps.frames && { frames: this.#deps.frames }),
        onAbort: (error) => {
          if (this.#drag === session) this.#drag = null;
          this.#notify(error);
        },
      });
      drag = session;
      this.#drag = session;
    }
    drag.update(step);
  }

  /** The pointer was released: one undo entry (none when nothing moved). */
  async commit(): Promise<void> {
    const drag = this.#drag;
    if (drag === null) return;
    this.#drag = null;
    const done = drag.finish(true).then((outcome) => {
      if (outcome.kind === "refused" || (outcome.kind === "aborted" && !outcome.reported)) {
        this.#notify(outcome.error);
      }
    });
    this.#queue = this.#queue.then(() => done);
    await done;
  }

  /** Escape, or the drag was abandoned: everything rolls back, no undo entry. */
  async cancel(): Promise<void> {
    const drag = this.#drag;
    if (drag === null) return;
    this.#drag = null;
    const done = drag.finish(false).then(() => undefined);
    this.#queue = this.#queue.then(() => done);
    await done;
  }

  /** Several edits as one batch (a key nudge): one undo entry named `label`. */
  apply(label: string, updates: readonly GestureUpdate[]): Promise<void> {
    const run = async (): Promise<void> => {
      const rpc = this.#rpc();
      if (rpc === null || updates.length === 0) return;
      let tx: TxId | null = null;
      try {
        tx = (await rpc.call("edit.begin_tx", { kind: "batch", label })).tx;
        for (const update of updates) {
          if (update.name === "item.set_props") await rpc.call(update.name, update.input, { tx });
        }
        await rpc.call("edit.commit_tx", { tx });
      } catch (error) {
        if (tx !== null) await rpc.call("edit.cancel_tx", { tx }).catch(() => undefined);
        this.#notify(error);
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
