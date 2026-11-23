// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import type { TimelineModel } from "./model.ts";
import { nearestTarget, SNAP_PX, snapSpan, snapTargets, snapTolerance } from "./snap.ts";
import { FLICKS_PER_SECOND as F, snapToFrame } from "./time.ts";

const fps = { num: 30, den: 1 };
const FRAME = F / 30;

describe("snapping helpers", () => {
  it("converts 8 px to flicks at the current zoom", () => {
    expect(SNAP_PX).toBe(8);
    expect(snapTolerance(100)).toBe(8 * (F / 100));
    expect(snapTolerance(800)).toBe(F / 100);
  });

  it("finds the nearest target on either side, preferring the earlier one on a tie", () => {
    const t = [0, 10, 20];
    expect(nearestTarget(t, 4)).toBe(0);
    expect(nearestTarget(t, 6)).toBe(10);
    expect(nearestTarget(t, 15)).toBe(10);
    expect(nearestTarget(t, 99)).toBe(20);
    expect(nearestTarget(t, -5)).toBe(0);
    expect(nearestTarget([], 3)).toBeNull();
  });

  it("collects edges once, sorted, without the excluded item, its group and with the playhead", () => {
    const model: TimelineModel = {
      fps,
      tracks: [],
      items: [
        { id: "a", trackId: "t", kind: "media", start: 10 * FRAME, duration: 5 * FRAME, label: "" },
        {
          id: "b",
          trackId: "t",
          kind: "media",
          start: 15 * FRAME,
          duration: 5 * FRAME,
          label: "",
          linkedGroup: "g",
        },
        { id: "c", trackId: "u", kind: "audio", start: 0, duration: 3 * FRAME, label: "" },
      ],
    };
    expect(snapTargets(model)).toEqual([0, 3 * FRAME, 10 * FRAME, 15 * FRAME, 20 * FRAME]);
    expect(
      snapTargets(model, { exclude: new Set(["a"]), excludeGroup: "g", playhead: 7 * FRAME + 3 }),
    ).toEqual([0, 3 * FRAME, 7 * FRAME]);
  });

  it("moves a span so its closest edge sits on a target within tolerance", () => {
    const targets = [0, 100 * FRAME];
    // A 10-frame span whose end lands 1 frame before the target at 100 frames.
    const r = snapSpan(89 * FRAME, [0, 10 * FRAME], targets, 2 * FRAME, fps);
    expect(r).toEqual({ at: 90 * FRAME, guide: 100 * FRAME });
  });

  it("rounds to the nearest frame without a target in reach, or without targets", () => {
    expect(snapSpan(50 * FRAME + 7, [0], [0], 2 * FRAME, fps)).toEqual({
      at: 50 * FRAME,
      guide: null,
    });
    expect(snapSpan(FRAME * 0.6, [0], [], 10 * FRAME, fps)).toEqual({ at: FRAME, guide: null });
  });

  it("rounds limits inward with ceil and floor", () => {
    expect(snapToFrame(FRAME + 1, fps, "ceil")).toBe(2 * FRAME);
    expect(snapToFrame(2 * FRAME, fps, "ceil")).toBe(2 * FRAME);
    expect(snapToFrame(2 * FRAME - 1, fps, "floor")).toBe(FRAME);
  });
});
