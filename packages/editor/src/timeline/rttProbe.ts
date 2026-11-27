// SPDX-License-Identifier: GPL-3.0-or-later
// The timeline's command round-trip probe (WP-2A.10): how long a drag update takes from leaving
// the UI to its `doc.patched` being applied in `docStore` (UI → engine → patch → UI). Each sample
// is a User Timing measure named `cutweave.timeline.rtt`, so it shows in the performance panel
// and a test reads it with `performance.getEntriesByName` (read-only; nothing changes behaviour).
// The entries of one gesture are kept until the next gesture starts, and the probe keeps its own
// last samples, so memory stays bounded however long the session.
import type { StoreApi } from "zustand/vanilla";
import type { DocStore } from "../state/docStore.ts";

/** The User Timing name of a sample. */
export const RTT_MEASURE = "cutweave.timeline.rtt";

/** Samples kept in memory (the probe's own copy). */
const KEEP = 256;

/** A patch that never comes (a no-op update) ends its sample after this long, unmeasured. */
const GIVE_UP_MS = 1000;

export interface RttToken {
  readonly t0: number;
  readonly rev0: number;
}

interface Clock {
  now(): number;
  measure?(name: string, options: { start: number; end: number }): unknown;
  clearMeasures?(name: string): void;
}

export class RttProbe {
  readonly #doc: StoreApi<Pick<DocStore, "rev">>;
  readonly #clock: Clock;
  readonly #samples: number[] = [];

  constructor(doc: StoreApi<Pick<DocStore, "rev">>, clock: Clock = globalThis.performance) {
    this.#doc = doc;
    this.#clock = clock;
  }

  /** A gesture starts: the previous gesture's measures are dropped. */
  reset(): void {
    this.#clock.clearMeasures?.(RTT_MEASURE);
  }

  /** An update leaves the UI. */
  start(): RttToken {
    return { t0: this.#clock.now(), rev0: this.#doc.getState().rev };
  }

  /**
   * The engine answered the update: the sample ends now if its patch is already applied, else
   * when the next one is.
   */
  answered(token: RttToken): void {
    if (this.#doc.getState().rev > token.rev0) {
      this.#record(token);
      return;
    }
    const timer = setTimeout(() => off(), GIVE_UP_MS);
    const off = this.#doc.subscribe((s) => {
      if (s.rev <= token.rev0) return;
      clearTimeout(timer);
      off();
      this.#record(token);
    });
  }

  /** The last samples, ms, oldest first. */
  samples(): readonly number[] {
    return this.#samples;
  }

  #record(token: RttToken): void {
    const end = this.#clock.now();
    this.#samples.push(end - token.t0);
    if (this.#samples.length > KEEP) this.#samples.shift();
    try {
      this.#clock.measure?.(RTT_MEASURE, { start: token.t0, end });
    } catch {
      // A clock without User Timing (an old runtime): the in-memory samples still count.
    }
  }
}

/** The `q` quantile (0..1) of `values` (nearest rank); `NaN` when empty. */
export function quantile(values: readonly number[], q: number): number {
  if (values.length === 0) return Number.NaN;
  const sorted = [...values].sort((a, b) => a - b);
  const rank = Math.min(sorted.length - 1, Math.max(0, Math.ceil(q * sorted.length) - 1));
  return sorted[rank] as number;
}
