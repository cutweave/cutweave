// SPDX-License-Identifier: GPL-3.0-or-later
// Snapping for timeline gestures (WP-1M.06): a dragged time is pulled to the playhead, the clip
// edges and the timeline start when it comes within `SNAP_PX` CSS pixels of one, and every result
// lands on the frame grid, so commands always carry frame-aligned times. Computed client-side in
// flicks (arch §16); the engine snaps to the same grid again, so nothing here is authoritative.
import type { TimelineModel } from "./model.ts";
import { FLICKS_PER_SECOND, type FrameRate, snapToFrame } from "./time.ts";

/** Snap distance, CSS pixels. */
export const SNAP_PX = 8;

/** Snap distance in flicks at zoom `pxPerSec`. */
export function snapTolerance(pxPerSec: number, px = SNAP_PX): number {
  return (px * FLICKS_PER_SECOND) / pxPerSec;
}

export interface SnapTargetOptions {
  /** Items whose edges are not targets (the dragged item). */
  readonly exclude?: ReadonlySet<string>;
  /** A linked group whose members are not targets (they move with the dragged item). */
  readonly excludeGroup?: string | undefined;
  /** The playhead, when it is a target (frame-aligned before use). */
  readonly playhead?: number | undefined;
}

/**
 * Sorted, de-duplicated snap targets: the timeline start, every item edge except the excluded
 * ones and, when given, the playhead on its frame. O(n log n); call once per gesture.
 */
export function snapTargets(model: TimelineModel, options: SnapTargetOptions = {}): number[] {
  const out: number[] = [0];
  const { exclude, excludeGroup } = options;
  for (const it of model.items) {
    if (exclude?.has(it.id)) continue;
    if (excludeGroup !== undefined && it.linkedGroup === excludeGroup) continue;
    out.push(it.start, it.start + it.duration);
  }
  if (options.playhead !== undefined) out.push(snapToFrame(options.playhead, model.fps));
  out.sort((a, b) => a - b);
  let n = 0;
  for (let i = 0; i < out.length; i++) {
    const v = out[i] as number;
    if (n === 0 || out[n - 1] !== v) out[n++] = v;
  }
  out.length = n;
  return out;
}

/** The target nearest to `t` in sorted `targets`, or null when there is none. */
export function nearestTarget(targets: readonly number[], t: number): number | null {
  if (targets.length === 0) return null;
  let lo = 0;
  let hi = targets.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if ((targets[mid] as number) < t) lo = mid + 1;
    else hi = mid;
  }
  const after = targets[lo];
  const before = targets[lo - 1];
  if (after === undefined) return before ?? null;
  if (before === undefined) return after;
  return t - before <= after - t ? before : after;
}

export interface SnapResult {
  /** The snapped, frame-aligned start of the span. */
  readonly at: number;
  /** The target an edge snapped to (draw it as a guide), or null on a plain frame snap. */
  readonly guide: number | null;
}

/**
 * Snaps a span that starts at `raw`. `offsets` are the span's edges relative to its start: `[0]`
 * for a single edge (a trim, the playhead), `[0, duration]` for a whole clip. The edge nearest to
 * a target within `tolerance` flicks wins and the span moves so that edge sits on the target;
 * without one (or with no targets: snapping off) the start rounds to the nearest frame. The
 * result is always frame-aligned.
 */
export function snapSpan(
  raw: number,
  offsets: readonly number[],
  targets: readonly number[],
  tolerance: number,
  fps: FrameRate,
): SnapResult {
  let best: number | null = null;
  let bestDist = tolerance;
  let bestOffset = 0;
  for (const off of offsets) {
    const target = nearestTarget(targets, raw + off);
    if (target === null) continue;
    const dist = Math.abs(target - (raw + off));
    if (dist <= bestDist && (best === null || dist < bestDist)) {
      best = target;
      bestDist = dist;
      bestOffset = off;
    }
  }
  if (best === null) return { at: snapToFrame(raw, fps), guide: null };
  return { at: snapToFrame(best - bestOffset, fps), guide: best };
}
