// SPDX-License-Identifier: GPL-3.0-or-later
// Shared snapping (WP-2A.07, timeline spec §6): thin wrappers over the engine's own `snap` from the
// math module (`@cutweave/protocol/wasm`, crates/cutweave-model/src/snap.rs), so a drag lands on
// exactly the frame the engine would snap to. Nothing here decides a snap; it only builds the
// candidates from the timeline projection and forwards the call. `initMath()` must have resolved
// (the host loads the module once at startup). The P1 `snapSpan` of `../snap.ts` stays for the
// callers that have not moved over yet (WP-2A.08).
import {
  type SnapCandidate,
  type SnapIndex,
  type SnapKindName,
  type SnapOptions as WasmSnapOptions,
  type SnapResult as WasmSnapResult,
  snap as wasm,
} from "@cutweave/protocol/wasm";
import type { TimelineModel } from "../model.ts";
import type { FrameRate } from "../time.ts";

export { SNAP_KINDS } from "@cutweave/protocol/wasm";
export type { SnapCandidate, SnapIndex, SnapKindName };

/** What a shared snap returns: the frame-aligned time and the candidate it landed on. */
export type SharedSnapResult = WasmSnapResult;

/** How a snap is asked for. */
export interface SnapRequest {
  /** The zoom, CSS pixels per second. */
  readonly pxPerSec: number;
  /** Snap distance in pixels (the Timeline preference "Snap distance"; default 8). */
  readonly thresholdPx?: number;
  /** The kinds that may attract (default all). */
  readonly enabled?: readonly SnapKindName[];
  /** Snapping is off (the toggle) or the override modifier (Ctrl/Cmd) is held: frame snap only. */
  readonly suspended?: boolean;
}

/** Candidates the timeline projection does not hold (markers, the sequence in/out). */
export interface SnapSources {
  /** The playhead, flicks. */
  readonly playhead?: number;
  /** Sequence and item markers (item markers already in timeline time), and range marker ends. */
  readonly markers?: readonly number[];
  /** Markers of kind `beat`. */
  readonly beats?: readonly number[];
  /** The sequence in and out. */
  readonly inOut?: { readonly in: number; readonly out: number } | null;
  /** Items whose edges are not candidates (the movers). */
  readonly exclude?: ReadonlySet<string>;
  /** A linked group whose members are not candidates (they move with the gesture). */
  readonly excludeGroup?: string | undefined;
}

/**
 * The candidates of spec §6: the playhead, the timeline start and the start and end of every item
 * (except the movers), markers, beats and the in/out. Times need not be on the frame grid; the
 * module moves each to its nearest frame.
 */
export function snapCandidates(model: TimelineModel, sources: SnapSources = {}): SnapCandidate[] {
  const out: SnapCandidate[] = [{ kind: "edge", t: 0 }];
  if (sources.playhead !== undefined) out.push({ kind: "playhead", t: sources.playhead });
  for (const it of model.items) {
    if (sources.exclude?.has(it.id)) continue;
    if (sources.excludeGroup !== undefined && it.linkedGroup === sources.excludeGroup) continue;
    out.push({ kind: "edge", t: it.start }, { kind: "edge", t: it.start + it.duration });
  }
  for (const t of sources.markers ?? []) out.push({ kind: "marker", t });
  for (const t of sources.beats ?? []) out.push({ kind: "beat", t });
  if (sources.inOut) {
    out.push({ kind: "in_out", t: sources.inOut.in }, { kind: "in_out", t: sources.inOut.out });
  }
  return out;
}

/** Builds the snap index of a sequence in the math module. Call `free()` when it is replaced. */
export function createSnapIndex(fps: FrameRate, candidates: readonly SnapCandidate[]): SnapIndex {
  return wasm.createIndex(fps, candidates);
}

/**
 * Snaps the raw time `raw` (flicks). `offsets` are the probes relative to `raw`: `[0]` for an
 * edge, the playhead or a marker; `[0, duration]` for a clip; every edge of every moving item
 * for a multi-item move. Without a candidate in reach (or suspended) the result is `raw` on the
 * nearest frame and `target` is null; the result is always frame-aligned.
 */
export function snap(
  index: SnapIndex,
  raw: number,
  offsets: readonly number[],
  request: SnapRequest,
): SharedSnapResult {
  const enabled = request.suspended === true ? [] : request.enabled;
  const options: WasmSnapOptions = {
    pxPerSecond: request.pxPerSec,
    ...(request.thresholdPx === undefined ? {} : { thresholdPx: request.thresholdPx }),
    ...(enabled === undefined ? {} : { enabled }),
  };
  return index.snap(raw, offsets, options);
}
