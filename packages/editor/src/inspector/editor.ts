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

import type {
  ItemProps,
  PropId,
  ResetGroup,
  RpcClient,
  TrackId,
  TrackSet,
  TxId,
} from "@cutweave/protocol";
import { deepEqual } from "@cutweave/ui";
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
import { keyTimes } from "./video.ts";

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

/** One property edit from the schema-driven Video sections (WP-2H.03a). */
export interface PropEdit {
  readonly clips: readonly SelectedClip[];
  /** Dot path inside `props` (`transform.position`); also the `PropId` of an animatable one. */
  readonly path: string;
  /** The new value of the property. */
  readonly value: unknown;
  /** `item.set_props` input `props`: only `path` set, the rest `null`. */
  readonly props: ItemProps;
  /** Sequence time (the playhead): an animated property is keyed there. */
  readonly at: number;
  /** The property's name for the undo entry and a refusal's toast ("position"). */
  readonly name: string;
  /** Per clip, the value now: a drag that ends here leaves no undo entry. */
  readonly current: readonly unknown[];
}

interface OpenDrag {
  readonly key: string;
  /** The undo entry's name. */
  readonly label: string;
  /** What a refusal's toast names. */
  readonly subject: string;
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

  #notify(subject: string, error: unknown): void {
    const failure = toRpcError(error);
    this.#deps.toasts.getState().push({
      tone: "danger",
      title: this.#deps.t("inspector.failed"),
      message: `${subject}: ${describeRefusal(failure, this.#deps.t)}`,
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
    if (field.control === "text") return;
    this.#live({
      key: `${field.id}:${clips.map((c) => c.item.id).join(",")}`,
      label: this.#deps.t(field.history),
      subject: this.#deps.t(field.label),
      clips,
      start: clips.map((c) => field.read(c.item)),
      value,
      step: this.#steps(field, clips, value) as GestureStep,
    });
  }

  /** The gesture of `live`, whatever the field: opens it, replaces its pending value. */
  #live(update: {
    key: string;
    label: string;
    subject: string;
    clips: readonly SelectedClip[];
    start: readonly unknown[];
    value: unknown;
    step: GestureStep;
  }): void {
    const rpc = this.#rpc();
    if (rpc === null || update.clips.length === 0) return;
    let drag = this.#drag;
    if (drag !== null && drag.key !== update.key) {
      // A drag of another field started before this one ended: end it first.
      void this.#end(drag, null);
      drag = null;
    }
    if (drag === null) {
      const session: GestureSession = new GestureSession({
        rpc,
        label: update.label,
        after: this.#queue,
        ...(this.#deps.frames && { frames: this.#deps.frames }),
        onAbort: (error) => {
          if (this.#drag?.session === session) this.#drag = null;
          this.#notify(update.subject, error);
        },
      });
      drag = {
        key: update.key,
        label: update.label,
        subject: update.subject,
        session,
        start: update.start,
        timer: null,
      };
      this.#drag = drag;
    }
    this.#clearTimer(drag);
    drag.session.update(update.step);
    // Dragged back to the start (or Escape, which reports the start value): no entry wanted.
    const atStart =
      update.clips.length === drag.start.length &&
      drag.start.every((from) => deepEqual(from, update.value));
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
    await this.#end(
      drag,
      clips.length > 0 ? (this.#steps(field, clips, value) as GestureStep) : null,
    );
  }

  /** Commits an open drag, with a last position first when there is one. */
  async #end(drag: OpenDrag, last: GestureStep | null): Promise<void> {
    if (this.#drag === drag) this.#drag = null;
    this.#clearTimer(drag);
    if (last !== null) drag.session.update(last);
    const done = drag.session.finish(true).then((outcome) => {
      if (outcome.kind === "refused" || (outcome.kind === "aborted" && !outcome.reported)) {
        this.#notify(drag.subject, outcome.error);
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
    if (clips.length === 0) return Promise.resolve();
    const updates = this.#steps(field, clips, value);
    return this.#batch(this.#deps.t(field.history), this.#deps.t(field.label), async (rpc, tx) => {
      for (const update of updates) await callUpdate(rpc, update, tx);
    });
  }

  /** Runs `work` inside one batch transaction (one undo entry); a refusal rolls it back. */
  #batch(
    label: string,
    subject: string,
    work: (rpc: RpcClient, tx: TxId) => Promise<void>,
  ): Promise<void> {
    const run = async (): Promise<void> => {
      const rpc = this.#rpc();
      if (rpc === null) return;
      let tx: TxId | null = null;
      try {
        const opened = await rpc.call("edit.begin_tx", { kind: "batch", label });
        tx = opened.tx;
        await work(rpc, tx);
        await rpc.call("edit.commit_tx", { tx });
      } catch (error) {
        if (tx !== null) await rpc.call("edit.cancel_tx", { tx }).catch(() => undefined);
        this.#notify(subject, error);
      }
    };
    const done = this.#queue.then(run, run);
    this.#queue = done;
    return done;
  }

  // ---- properties of the Video sections -----------------------------------------------------

  #propStep(edit: PropEdit): GestureUpdate {
    return {
      name: "item.set_props",
      input: { items: edit.clips.map((c) => c.item.id), props: edit.props, at: edit.at },
    };
  }

  #propKey(edit: PropEdit): string {
    return `prop:${edit.path}:${edit.clips.map((c) => c.item.id).join(",")}`;
  }

  /** A live value of a drag on a property: opens the gesture, later calls replace the value. */
  liveProp(edit: PropEdit): void {
    this.#live({
      key: this.#propKey(edit),
      label: this.#deps.t("inspector.history.change", { name: edit.name }),
      subject: edit.name,
      clips: edit.clips,
      start: edit.current,
      value: edit.value,
      step: this.#propStep(edit),
    });
  }

  /** The drag ended (or a key press committed): one undo entry, none when it ended where it began. */
  async commitProp(edit: PropEdit): Promise<void> {
    const drag = this.#drag;
    if (drag === null || drag.key !== this.#propKey(edit)) {
      await this.applyProp(edit);
      return;
    }
    if (drag.start.every((from) => deepEqual(from, edit.value))) {
      await this.cancel();
      return;
    }
    await this.#end(drag, this.#propStep(edit));
  }

  /** A typed, picked or toggled value: one undo entry. */
  applyProp(edit: PropEdit): Promise<void> {
    if (edit.clips.length === 0) return Promise.resolve();
    return this.#batch(
      this.#deps.t("inspector.history.change", { name: edit.name }),
      edit.name,
      async (rpc, tx) => {
        const step = this.#propStep(edit);
        if (step.name === "item.set_props") await rpc.call(step.name, step.input, { tx });
      },
    );
  }

  /** Puts properties (or a whole section) back to their defaults, keys removed: one undo entry. */
  resetProps(
    clips: readonly SelectedClip[],
    what: { readonly props: readonly PropId[] } | { readonly group: ResetGroup },
    name: string,
  ): Promise<void> {
    if (clips.length === 0) return Promise.resolve();
    const items = clips.map((c) => c.item.id);
    return this.#batch(this.#deps.t("inspector.history.reset", { name }), name, async (rpc, tx) => {
      await rpc.call(
        "item.reset_props",
        "props" in what ? { items, props: [...what.props] } : { items, group: what.group },
        { tx },
      );
    });
  }

  /**
   * The diamond: adds a key at the playhead (holding the value there) on the clips, or removes
   * the keys the playhead sits on.
   */
  toggleKey(
    clips: readonly SelectedClip[],
    prop: PropId,
    at: number,
    remove: boolean,
    name: string,
  ): Promise<void> {
    if (clips.length === 0) return Promise.resolve();
    const label = this.#deps.t(
      remove ? "inspector.history.removeKey" : "inspector.history.addKey",
      { name },
    );
    return this.#batch(label, name, async (rpc, tx) => {
      if (remove) {
        for (const { item } of clips) {
          const keys = keyTimesAt(item, prop, at);
          if (keys.length > 0) await rpc.call("keyframe.delete", { item: item.id, keys }, { tx });
        }
        return;
      }
      await rpc.call(
        "keyframe.set",
        {
          items: clips.map((c) => c.item.id),
          prop,
          keys: [{ t: null, at, value: null, interp: null, easing: null, preset: null }],
        },
        { tx },
      );
    });
  }

  /** A track's name, flag or gain (`track.set`): one undo entry. */
  setTrack(track: TrackId, patch: Omit<TrackSet, "track">, name: string): Promise<void> {
    return this.#batch(this.#deps.t("inspector.history.track", { name }), name, async (rpc, tx) => {
      await rpc.call("track.set", { track, ...patch }, { tx });
    });
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

/** The `KeyRef`s of `prop` on keys that sit at the sequence time `at`. */
function keyTimesAt(item: SelectedClip["item"], prop: PropId, at: number) {
  return (keyTimes(item, prop) ?? [])
    .filter((t) => item.start + t === at)
    .map((t) => ({ prop, t }));
}
