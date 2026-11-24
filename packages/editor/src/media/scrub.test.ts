// SPDX-License-Identifier: GPL-3.0-or-later
// WP-1M.14: pointer position -> media time on the frame grid, and latest-wins loading.
import { FLICKS_PER_SECOND, frameDurationFlicks } from "@cutweave/timeline";
import { describe, expect, it } from "vitest";
import {
  fractionOfFrame,
  frameAtFraction,
  frameGrid,
  LatestFrames,
  pointerFraction,
  timeOfFrame,
} from "./scrub.ts";

const FPS30 = { num: 30, den: 1 };
const NTSC = { num: 30000, den: 1001 };

describe("the frame grid", () => {
  it("counts the frames of an asset", () => {
    expect(frameGrid(5 * FLICKS_PER_SECOND, FPS30).frames).toBe(150);
    expect(frameGrid(8 * FLICKS_PER_SECOND, { num: 60, den: 1 }).frames).toBe(480);
    // A length that ends inside a frame still shows that frame.
    expect(frameGrid(frameDurationFlicks(NTSC) * 10 + 5, NTSC).frames).toBe(11);
  });

  it("falls back to 30 fps and one frame for what has no rate or length", () => {
    expect(frameGrid(FLICKS_PER_SECOND, null).rate).toEqual(FPS30);
    expect(frameGrid(null, FPS30).frames).toBe(1);
    expect(frameGrid(0, FPS30).frames).toBe(1);
  });

  it("maps the pointer to a frame at nine fractions, and the frame to its start time", () => {
    const grid = frameGrid(5 * FLICKS_PER_SECOND, FPS30);
    const step = frameDurationFlicks(FPS30);
    const fractions = [0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1];
    const frames = fractions.map((f) => frameAtFraction(f, grid));
    expect(frames).toEqual([0, 18, 37, 56, 75, 93, 112, 131, 149]);
    for (const [i, frame] of frames.entries()) {
      const t = timeOfFrame(frame, grid);
      // On the grid, and never past the length.
      expect(t % step).toBe(0);
      expect(t).toBeLessThan(5 * FLICKS_PER_SECOND);
      expect(t).toBe(frame * step);
      if (i > 0) expect(frame).toBeGreaterThanOrEqual(frames[i - 1] ?? 0);
    }
    // Half way is exactly half the length.
    expect(timeOfFrame(75, grid)).toBe(2.5 * FLICKS_PER_SECOND);
  });

  it("clamps what lies outside the box", () => {
    const grid = frameGrid(FLICKS_PER_SECOND, FPS30);
    expect(frameAtFraction(-3, grid)).toBe(0);
    expect(frameAtFraction(7, grid)).toBe(29);
    expect(frameAtFraction(Number.NaN, grid)).toBe(0);
    expect(timeOfFrame(99, grid)).toBe(29 * frameDurationFlicks(FPS30));
    expect(pointerFraction(50, 100, 200)).toBe(0);
    expect(pointerFraction(150, 100, 200)).toBe(0.25);
    expect(pointerFraction(900, 100, 200)).toBe(1);
    expect(pointerFraction(5, 0, 0)).toBe(0);
    expect(fractionOfFrame(0, grid)).toBeGreaterThan(0);
    expect(fractionOfFrame(29, grid)).toBeLessThan(1);
  });
});

/** A manual animation-frame clock and a fake source whose answers the test releases. */
function rig() {
  let next = 1;
  const queue = new Map<number, () => void>();
  const asked: { t: number; signal: AbortSignal; answer: (v: string | null) => void }[] = [];
  const painted: { t: number; picture: string }[] = [];
  const frames = new LatestFrames<string>({
    raf: (run) => {
      const handle = next++;
      queue.set(handle, run);
      return handle;
    },
    cancelRaf: (handle) => queue.delete(handle),
    load: (t, signal) =>
      new Promise<string | null>((resolve) => {
        asked.push({ t, signal, answer: resolve });
      }),
    paint: (t, picture) => painted.push({ t, picture }),
  });
  const frame = () => {
    const runs = [...queue.values()];
    queue.clear();
    for (const run of runs) run();
  };
  return { frames, frame, asked, painted, scheduled: () => queue.size };
}

describe("LatestFrames", () => {
  it("asks for at most one frame per animation frame", () => {
    const { frames, frame, asked } = rig();
    for (let t = 1; t <= 20; t++) frames.request(t);
    expect(asked).toHaveLength(0);
    frame();
    expect(asked.map((a) => a.t)).toEqual([20]);
    for (let t = 21; t <= 30; t++) frames.request(t);
    frame();
    expect(asked.map((a) => a.t)).toEqual([20, 30]);
    frame();
    expect(asked).toHaveLength(2);
  });

  it("aborts the request in flight when a newer one starts", () => {
    const { frames, frame, asked } = rig();
    frames.request(1);
    frame();
    frames.request(2);
    frame();
    expect(asked[0]?.signal.aborted).toBe(true);
    expect(asked[1]?.signal.aborted).toBe(false);
  });

  it("never paints an answer that arrives after a newer one", async () => {
    const { frames, frame, asked, painted } = rig();
    frames.request(1);
    frame();
    frames.request(2);
    frame();
    // The newer answer first, then the late one for t = 1.
    asked[1]?.answer("two");
    await Promise.resolve();
    asked[0]?.answer("one");
    await Promise.resolve();
    await Promise.resolve();
    expect(painted).toEqual([{ t: 2, picture: "two" }]);
  });

  it("paints an answer that is the newest even if it is slow", async () => {
    const { frames, frame, asked, painted } = rig();
    frames.request(4);
    frame();
    asked[0]?.answer("four");
    await Promise.resolve();
    await Promise.resolve();
    expect(painted).toEqual([{ t: 4, picture: "four" }]);
  });

  it("asks nothing and paints nothing after the pointer leaves", async () => {
    const { frames, frame, asked, painted, scheduled } = rig();
    frames.request(1);
    frame();
    frames.request(2);
    expect(scheduled()).toBe(1);
    frames.cancel();
    expect(scheduled()).toBe(0);
    frame();
    expect(asked).toHaveLength(1);
    expect(asked[0]?.signal.aborted).toBe(true);
    asked[0]?.answer("late");
    await Promise.resolve();
    await Promise.resolve();
    expect(painted).toEqual([]);
  });

  it("asks again after a leave for the same position", () => {
    const { frames, frame, asked } = rig();
    frames.request(7);
    frame();
    frames.cancel();
    frames.request(7);
    frame();
    expect(asked.map((a) => a.t)).toEqual([7, 7]);
  });

  it("does not ask twice for the position it already has", () => {
    const { frames, frame, asked } = rig();
    frames.request(7);
    frame();
    frames.request(7);
    frame();
    expect(asked).toHaveLength(1);
  });
});
