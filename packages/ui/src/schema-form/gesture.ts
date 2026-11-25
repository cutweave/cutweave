// SPDX-License-Identifier: GPL-3.0-or-later
import type { ChangeMeta, OnChange } from "./types.ts";

/** A gesture streams at most this many `update` callbacks per second. */
export const MAX_UPDATES_PER_SECOND = 60;
// Rounded up, so no one-second window can hold more than 60 updates even when a timer fires early.
const MIN_INTERVAL_MS = Math.ceil(1000 / MAX_UPDATES_PER_SECOND) + 1;

export interface Clock {
  now: () => number;
  setTimeout: (fn: () => void, ms: number) => unknown;
  clearTimeout: (handle: unknown) => void;
}

const REAL_CLOCK: Clock = {
  now: () => performance.now(),
  setTimeout: (fn, ms) => setTimeout(fn, ms),
  clearTimeout: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

interface Slot {
  phase: "held" | "active";
  /** The value before the gesture, restored on cancel. */
  origin: unknown;
  lastEmit: number;
  pending: { value: unknown } | null;
  /** The latest value seen. */
  last: unknown;
  timer: unknown;
}

/**
 * Turns the raw change stream of a control (many `change`s, one `commit`) into the form's
 * callback contract:
 *
 * - a lone change followed by a commit (a key press, a click, a typed value) is **one-shot**:
 *   a single `onChange(path, value, {})`;
 * - a drag is `start` (first streamed value), throttled `update`s (at most 60 per second, the
 *   latest value always wins), and exactly one `end` carrying the final value;
 * - `cancel` (Escape mid-drag) closes an open gesture with `end` at the original value, so the
 *   host's transaction never stays open.
 */
export class GestureBus {
  private readonly slots = new Map<string, Slot>();

  constructor(
    private readonly emit: () => OnChange,
    private readonly draft: (path: string, value: unknown, active: boolean) => void,
    private readonly clock: Clock = REAL_CLOCK,
  ) {}

  private send(path: string, value: unknown, meta: ChangeMeta) {
    this.emit()(path, value, meta);
  }

  /** A live value; `origin` is the value before the gesture (for `cancel`). */
  change(path: string, value: unknown, origin?: unknown) {
    this.draft(path, value, true);
    const slot = this.slots.get(path);
    if (!slot) {
      this.slots.set(path, {
        phase: "held",
        origin,
        lastEmit: 0,
        pending: { value },
        last: value,
        timer: null,
      });
      return;
    }
    slot.last = value;
    const now = this.clock.now();
    if (slot.phase === "held") {
      slot.phase = "active";
      slot.pending = null;
      slot.lastEmit = now;
      this.send(path, value, { gesture: "start" });
      return;
    }
    const wait = slot.lastEmit + MIN_INTERVAL_MS - now;
    if (wait <= 0) {
      this.flushUpdate(path, slot, value, now);
      return;
    }
    slot.pending = { value };
    if (slot.timer === null) {
      slot.timer = this.clock.setTimeout(() => {
        slot.timer = null;
        const pending = slot.pending;
        if (pending && this.slots.get(path) === slot) {
          this.flushUpdate(path, slot, pending.value, this.clock.now());
        }
      }, wait);
    }
  }

  private flushUpdate(path: string, slot: Slot, value: unknown, now: number) {
    slot.pending = null;
    slot.lastEmit = now;
    this.send(path, value, { gesture: "update" });
  }

  /** The end of a gesture, or a discrete change. */
  commit(path: string, value: unknown) {
    const slot = this.slots.get(path);
    this.slots.delete(path);
    if (slot?.timer != null) this.clock.clearTimeout(slot.timer);
    this.draft(path, value, false);
    this.send(path, value, slot?.phase === "active" ? { gesture: "end" } : {});
  }

  /** The user abandoned the gesture. */
  cancel(path: string) {
    const slot = this.slots.get(path);
    if (!slot) return;
    this.slots.delete(path);
    if (slot.timer != null) this.clock.clearTimeout(slot.timer);
    this.draft(path, slot.origin, false);
    if (slot.phase === "active") this.send(path, slot.origin, { gesture: "end" });
  }

  /** The form unmounts mid-gesture: close every open gesture at its latest value. */
  dispose() {
    for (const [path, slot] of this.slots) {
      if (slot.timer != null) this.clock.clearTimeout(slot.timer);
      if (slot.phase === "active") this.send(path, slot.last, { gesture: "end" });
    }
    this.slots.clear();
  }
}
