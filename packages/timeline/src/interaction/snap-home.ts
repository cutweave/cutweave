// SPDX-License-Identifier: GPL-3.0-or-later
// Snapping a dragged time that has a home (issue #263): where the clip, the trim edge, the cut,
// the keyframe or the marker was when the drag started.
import { type FrameRate, snapToFrame } from "../time.ts";
import type { SnapOutcome } from "./types.ts";

/**
 * The outcome of snapping `raw`, for a drag whose time started at `home`. A snap never pulls
 * further than the pointer has carried the time from `home`: a candidate counts only once it is
 * at least as near as `home` is. So the pointer back at the press point leaves the time exactly
 * where it was, whatever lies within the snap distance, and close to it the time follows the
 * pointer on the frame grid, as it does with nothing to snap to.
 */
export function snapFromHome(
  snapped: SnapOutcome,
  home: number,
  raw: number,
  fps: FrameRate,
): SnapOutcome {
  if (raw === home) return { t: home, target: null };
  if (snapped.target === null || Math.abs(snapped.t - raw) <= Math.abs(raw - home)) return snapped;
  return { t: snapToFrame(raw, fps), target: null };
}
