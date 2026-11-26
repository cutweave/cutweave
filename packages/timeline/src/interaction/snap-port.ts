// SPDX-License-Identifier: GPL-3.0-or-later
// The machine's snapping through the shared math module (WP-2A.07, spec §6): one wasm
// `SnapIndex` whose candidate kinds are replaced only when what they come from changes. A drag
// queries with the snapshot it started from and the set of its movers, so the edge kind is
// rebuilt once at the start of the drag (without the movers) and once after it; hovering and
// scrubbing rebuild it once per document patch. `initMath()` must have resolved.
import type { TimelineModel } from "../model.ts";
import { createSnapIndex, type SnapIndex, snap } from "../snap/index.ts";
import type { SnapOutcome, SnapPort, SnapQuery } from "./types.ts";

export interface WasmSnapPort extends SnapPort {
  /** Releases the module's index. */
  dispose(): void;
}

const UNSET = Symbol("unset");

/** Every item edge and the timeline start, minus the excluded items' (spec §6). */
function edgeTimes(model: TimelineModel, exclude: ReadonlySet<string> | null): number[] {
  const out = [0];
  for (const it of model.items) {
    if (exclude?.has(it.id) === true) continue;
    out.push(it.start, it.start + it.duration);
  }
  return out;
}

export function createSnapPort(): WasmSnapPort {
  let index: SnapIndex | null = null;
  let rate: TimelineModel["fps"] | null = null;
  let edgesModel: TimelineModel | typeof UNSET = UNSET;
  let edgesExclude: ReadonlySet<string> | null | typeof UNSET = UNSET;
  let markers: TimelineModel["markers"] | typeof UNSET = UNSET;
  let playhead: number | typeof UNSET = UNSET;
  let inOut: SnapQuery["inOut"] | typeof UNSET = UNSET;

  const reset = (q: SnapQuery): SnapIndex => {
    index?.free();
    rate = q.model.fps;
    edgesModel = UNSET;
    edgesExclude = UNSET;
    markers = UNSET;
    playhead = UNSET;
    inOut = UNSET;
    index = createSnapIndex(q.model.fps, []);
    return index;
  };

  return {
    snap(q: SnapQuery): SnapOutcome {
      const sameRate =
        rate !== null && rate.num === q.model.fps.num && rate.den === q.model.fps.den;
      const idx = index !== null && sameRate ? index : reset(q);
      if (edgesModel !== q.model || edgesExclude !== q.exclude) {
        idx.setKind("edge", edgeTimes(q.model, q.exclude));
        edgesModel = q.model;
        edgesExclude = q.exclude;
      }
      if (markers !== q.model.markers) {
        const marks: number[] = [];
        const beats: number[] = [];
        for (const m of q.model.markers ?? []) {
          const into = m.kind === "beat" ? beats : marks;
          into.push(m.t);
          if (m.duration !== undefined && m.duration !== null && m.duration > 0) {
            into.push(m.t + m.duration);
          }
        }
        idx.setKind("marker", marks);
        idx.setKind("beat", beats);
        markers = q.model.markers;
      }
      if (playhead !== q.playhead) {
        idx.setKind("playhead", [q.playhead]);
        playhead = q.playhead;
      }
      if (inOut !== q.inOut) {
        idx.setKind("in_out", q.inOut === null ? [] : [q.inOut.in, q.inOut.out]);
        inOut = q.inOut;
      }
      const r = snap(idx, q.raw, q.offsets, {
        pxPerSec: q.pxPerSec,
        thresholdPx: q.thresholdPx,
        enabled: q.kinds,
        suspended: q.suspended,
      });
      return { t: r.t, target: r.target };
    },
    dispose() {
      index?.free();
      index = null;
      rate = null;
    },
  };
}
