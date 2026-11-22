// SPDX-License-Identifier: GPL-3.0-or-later
// Hit-testing: canvas point -> what is under it. Pure over the layout and viewport; uses the same
// per-track interval index and the same geometry and LOD rules as the renderer, so a hit always
// matches what is drawn (for example keyframes are hittable only when they are drawn).
import {
  HANDLE_PX,
  HEADER_TOGGLES,
  type HeaderToggle,
  KEYFRAME_MIN_PX,
  keyframeRowY,
  MIN_HIT_PX,
  showsKeyframes,
  toggleRect,
} from "./geometry.ts";
import type { TimelineLayout } from "./layout.ts";
import type { Theme } from "./theme.ts";
import { FLICKS_PER_SECOND } from "./time.ts";
import { type Viewport, xToTime } from "./viewport.ts";

export type HitZone =
  /** Inside a clip, away from its edges. */
  | "body"
  /** Trim handle zone at a clip's start edge. */
  | "left-edge"
  /** Trim handle zone at a clip's end edge. */
  | "right-edge"
  /** A keyframe diamond of a clip (`keyframeIndex`, `keyframeTime`). */
  | "keyframe"
  /** The playhead flag in the ruler. */
  | "playhead"
  /** The ruler (scrub target). */
  | "ruler"
  /** A track header (`trackId`). */
  | "header"
  /** A header toggle button (`toggle`). */
  | "header-toggle"
  /** Empty lane space on a track (`trackId`, `time`): drop and marquee target. */
  | "lane"
  /** Outside every region (below the last track, the ruler corner). */
  | "none";

export interface HitResult {
  readonly zone: HitZone;
  /** Timeline time under the pointer, integer flicks (also set over headers and `none`). */
  readonly time: number;
  readonly trackId: string | null;
  readonly trackIndex: number;
  readonly itemId: string | null;
  readonly itemIndex: number;
  /** Set when `zone === "keyframe"`. */
  readonly keyframeIndex: number;
  readonly keyframeTime: number | null;
  /** Set when `zone === "header-toggle"`. */
  readonly toggle: HeaderToggle | null;
}

export interface HitContext {
  readonly layout: TimelineLayout;
  readonly vp: Viewport;
  readonly playhead: number;
  readonly theme: Pick<Theme, "controlSm" | "space1" | "space1_5" | "space2" | "trackSm">;
  /** Half width of the playhead flag in CSS px (as last drawn). */
  readonly flagHalfWidth: number;
  /** Reused candidate buffer; pass the same array every call to avoid allocation. */
  readonly scratch?: number[];
}

function result(partial: Partial<HitResult> & Pick<HitResult, "zone" | "time">): HitResult {
  return {
    trackId: null,
    trackIndex: -1,
    itemId: null,
    itemIndex: -1,
    keyframeIndex: -1,
    keyframeTime: null,
    toggle: null,
    ...partial,
  };
}

/** First index in sorted `arr` with value >= `t`. */
function lowerBound(arr: readonly number[], t: number): number {
  let lo = 0;
  let hi = arr.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if ((arr[mid] ?? 0) < t) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

export function hitTest(ctx: HitContext, x: number, y: number): HitResult {
  const { layout, vp, theme } = ctx;
  const time = xToTime(vp, x);
  if (x < 0 || y < 0 || x >= vp.width || y >= vp.height) return result({ zone: "none", time });

  // Ruler: the playhead flag wins over the plain ruler.
  if (y < vp.rulerHeight) {
    if (x < vp.headerWidth) return result({ zone: "none", time });
    const phx = vp.headerWidth + (ctx.playhead - vp.scrollX) * (vp.pxPerSec / FLICKS_PER_SECOND);
    if (Math.abs(x - phx) <= ctx.flagHalfWidth) return result({ zone: "playhead", time });
    return result({ zone: "ruler", time });
  }

  const contentY = y - vp.rulerHeight + vp.scrollY;
  const ti = layout.trackAtContentY(contentY);
  if (ti < 0) return result({ zone: "none", time });
  const trackId = layout.model.tracks[ti]?.id ?? null;
  const laneTop = vp.rulerHeight + (layout.trackTop[ti] ?? 0) - vp.scrollY;
  const laneH = layout.trackHeight[ti] ?? 0;

  if (x < vp.headerWidth) {
    for (let i = 0; i < HEADER_TOGGLES.length; i++) {
      const r = toggleRect(vp, theme, laneTop, laneH, i);
      if (x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h) {
        return result({
          zone: "header-toggle",
          time,
          trackId,
          trackIndex: ti,
          toggle: HEADER_TOGGLES[i] ?? null,
        });
      }
    }
    return result({ zone: "header", time, trackId, trackIndex: ti });
  }

  const clipTop = laneTop + layout.pad;
  const clipH = layout.clipHeight(ti);
  const emptyLane = result({ zone: "lane", time, trackId, trackIndex: ti });
  if (y < clipTop || y >= clipTop + clipH) return emptyLane;

  const k = vp.pxPerSec / FLICKS_PER_SECOND;
  const tol = MIN_HIT_PX / k;
  const ids = layout.query(ti, time - tol, time + tol, ctx.scratch ?? []);
  // Later items (greater start) are drawn on top, so test them first.
  for (let n = ids.length - 1; n >= 0; n--) {
    const i = ids[n] as number;
    let x0 = vp.headerWidth + ((layout.start[i] ?? 0) - vp.scrollX) * k;
    let x1 = vp.headerWidth + ((layout.end[i] ?? 0) - vp.scrollX) * k;
    const w = x1 - x0;
    if (w < MIN_HIT_PX) {
      const grow = (MIN_HIT_PX - w) / 2;
      x0 -= grow;
      x1 += grow;
    }
    if (x < x0 || x >= x1) continue;
    const item = layout.items[i];
    if (!item) continue;
    const base = { time, trackId, trackIndex: ti, itemId: item.id, itemIndex: i };

    const kfs = item.keyframes;
    if (kfs && kfs.length > 0 && w >= KEYFRAME_MIN_PX && showsKeyframes(clipH, theme)) {
      const reach = theme.space1 + 2;
      if (Math.abs(y - keyframeRowY(theme, clipTop, clipH)) <= reach) {
        const j = lowerBound(kfs, time);
        let best = -1;
        let bestDx = reach + 1;
        for (const c of [j - 1, j]) {
          const kt = kfs[c];
          if (kt === undefined) continue;
          const dx = Math.abs(vp.headerWidth + (kt - vp.scrollX) * k - x);
          if (dx < bestDx) {
            bestDx = dx;
            best = c;
          }
        }
        if (best >= 0 && bestDx <= reach) {
          return result({
            zone: "keyframe",
            ...base,
            keyframeIndex: best,
            keyframeTime: kfs[best] ?? null,
          });
        }
      }
    }

    const handle = Math.min(HANDLE_PX, Math.floor(w / 3));
    if (handle >= 2) {
      if (x - x0 < handle) return result({ zone: "left-edge", ...base });
      if (x1 - x <= handle) return result({ zone: "right-edge", ...base });
    }
    return result({ zone: "body", ...base });
  }
  return emptyLane;
}
