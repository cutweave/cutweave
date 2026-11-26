// SPDX-License-Identifier: GPL-3.0-or-later
// WP-2A.07: the timeline's shared snapping runs the engine's code through the math module.

import { mathModuleBytes } from "@cutweave/protocol/test-support/math-wasm";
import { initMath } from "@cutweave/protocol/wasm";
import { beforeAll, describe, expect, it } from "vitest";
import type { TimelineItem, TimelineModel } from "../model.ts";
import { snapSpan, snapTargets, snapTolerance } from "../snap.ts";
import { FLICKS_PER_SECOND as S } from "../time.ts";
import { createSnapIndex, type SnapIndex, snap, snapCandidates } from "./index.ts";

const fps = { num: 25, den: 1 };
const F = S / 25;

const item = (id: string, start: number, duration: number, group?: string): TimelineItem => ({
  id,
  trackId: "v1",
  kind: "media",
  start,
  duration,
  label: id,
  ...(group === undefined ? {} : { linkedGroup: group }),
});

const model: TimelineModel = {
  fps,
  tracks: [],
  items: [item("a", 10 * F, 20 * F), item("b", 40 * F, 10 * F, "g"), item("c", 60 * F, 5 * F, "g")],
};

beforeAll(async () => {
  await initMath(mathModuleBytes());
}, 900_000);

describe("snapCandidates", () => {
  it("collects the start, every edge, the playhead, markers, beats and the range", () => {
    const list = snapCandidates(model, {
      playhead: 7,
      markers: [3],
      beats: [4],
      inOut: { in: 1, out: 2 },
    });
    expect(list.map((c) => `${c.kind}:${String(c.t)}`).sort()).toEqual(
      [
        "edge:0",
        `edge:${10 * F}`,
        `edge:${30 * F}`,
        `edge:${40 * F}`,
        `edge:${50 * F}`,
        `edge:${60 * F}`,
        `edge:${65 * F}`,
        "playhead:7",
        "marker:3",
        "beat:4",
        "in_out:1",
        "in_out:2",
      ].sort(),
    );
  });

  it("leaves out the movers and their linked group", () => {
    const list = snapCandidates(model, { exclude: new Set(["a"]), excludeGroup: "g" });
    expect(list).toEqual([{ kind: "edge", t: 0 }]);
  });
});

describe("snap through the math module", () => {
  let index: SnapIndex;
  beforeAll(() => {
    index = createSnapIndex(
      fps,
      snapCandidates(model, { playhead: 20 * F, markers: [35 * F], beats: [70 * F] }),
    );
  });

  it("snaps to the playhead, an edge, a marker and a beat, on the frame grid", () => {
    const req = { pxPerSec: 100 };
    expect(snap(index, 20 * F + 5000, [0], req)).toEqual({
      t: 20 * F,
      target: { kind: "playhead", t: 20 * F },
      probe: 0,
    });
    expect(snap(index, 10 * F - 5000, [0], req).target?.kind).toBe("edge");
    expect(snap(index, 35 * F + 5000, [0], req).target?.kind).toBe("marker");
    expect(snap(index, 70 * F + 5000, [0], req).target?.kind).toBe("beat");
  });

  it("snaps a clip by its end and reports the probe", () => {
    const got = snap(index, 26 * F + 1000, [0, 4 * F], { pxPerSec: 100 });
    expect(got).toEqual({ t: 26 * F, target: { kind: "edge", t: 30 * F }, probe: 1 });
  });

  it("is the frame snap when suspended, with a kind disabled or out of reach", () => {
    expect(snap(index, 20 * F + 5000, [0], { pxPerSec: 100, suspended: true })).toEqual({
      t: 20 * F,
      target: null,
      probe: 0,
    });
    expect(snap(index, 20 * F + 5000, [0], { pxPerSec: 100, enabled: ["beat"] }).target).toBeNull();
    expect(snap(index, 20 * F + 100_000, [0], { pxPerSec: 100_000 }).target).toBeNull();
    expect(snap(index, 20 * F + F / 2 + 1, [0], { pxPerSec: 1, enabled: [] }).t).toBe(21 * F);
  });

  it("scales the tolerance with the zoom and honours the snap distance", () => {
    const near = 20 * F + snapTolerance(100) - 1;
    expect(snap(index, near, [0], { pxPerSec: 100 }).target).not.toBeNull();
    expect(snap(index, near, [0], { pxPerSec: 200 }).target).toBeNull();
    expect(snap(index, near, [0], { pxPerSec: 200, thresholdPx: 16 }).target).not.toBeNull();
  });

  it("agrees with the P1 snapSpan on playhead and edge candidates (INV-324)", () => {
    const edges = snapCandidates(model, { playhead: 20 * F });
    const idx = createSnapIndex(fps, edges);
    const targets = snapTargets(model, { playhead: 20 * F });
    let seed = 0x2a07;
    const next = (n: number) => {
      seed = (Math.imul(seed, 1103515245) + 12345) >>> 0;
      return seed % n;
    };
    for (let i = 0; i < 500; i++) {
      const raw = next(70 * F);
      const offsets = [0, (1 + next(20)) * F];
      const pxPerSec = [20, 100, 400][next(3)] as number;
      const want = snapSpan(raw, offsets, targets, snapTolerance(pxPerSec), fps);
      const got = snap(idx, raw, offsets, { pxPerSec });
      if (want.at >= 0) {
        expect(got.t, `raw ${raw} at ${pxPerSec} px/s`).toBe(want.at);
      }
    }
    idx.free();
  });

  it("frees an index", () => {
    const own = createSnapIndex(fps, [{ kind: "edge", t: 0 }]);
    own.free();
    expect(() => snap(own, 0, [0], { pxPerSec: 100 })).toThrow(/freed/);
  });
});
