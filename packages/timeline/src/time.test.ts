// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from "vitest";
import {
  FLICKS_PER_SECOND,
  type FrameRate,
  flicksToFrame,
  formatTimecode,
  frameDurationFlicks,
  frameToFlicks,
  snapToFrame,
} from "./time.ts";

const RATES: Array<[string, FrameRate]> = [
  ["23.976", { num: 24000, den: 1001 }],
  ["24", { num: 24, den: 1 }],
  ["25", { num: 25, den: 1 }],
  ["29.97", { num: 30000, den: 1001 }],
  ["30", { num: 30, den: 1 }],
  ["50", { num: 50, den: 1 }],
  ["59.94", { num: 60000, den: 1001 }],
  ["60", { num: 60, den: 1 }],
  ["120", { num: 120, den: 1 }],
];

describe("frame math", () => {
  it("has the exact integer frame durations", () => {
    expect(FLICKS_PER_SECOND).toBe(705_600_000);
    expect(frameDurationFlicks({ num: 24, den: 1 })).toBe(29_400_000);
    expect(frameDurationFlicks({ num: 24000, den: 1001 })).toBe(29_429_400);
    expect(frameDurationFlicks({ num: 30000, den: 1001 })).toBe(23_543_520);
  });

  it.each(RATES)("round-trips frames and flicks exactly at %s fps", (_n, fps) => {
    for (let f = 0; f < 1_000_000; f += 997) {
      const t = frameToFlicks(f, fps);
      expect(flicksToFrame(t, fps)).toBe(f);
      expect(flicksToFrame(t - 1, fps)).toBe(f - 1);
      expect(flicksToFrame(t + frameDurationFlicks(fps) - 1, fps)).toBe(f);
    }
  });

  it("snaps to the nearest or previous frame", () => {
    const fps = { num: 24, den: 1 };
    const d = frameDurationFlicks(fps);
    expect(snapToFrame(d + 1, fps)).toBe(d);
    expect(snapToFrame(d + d / 2, fps)).toBe(2 * d);
    expect(snapToFrame(2 * d - 1, fps, "floor")).toBe(d);
  });
});

describe("timecode", () => {
  it("formats non-drop timecode", () => {
    const fps = { num: 24, den: 1 };
    expect(formatTimecode(0, fps)).toBe("00:00:00:00");
    expect(formatTimecode(frameToFlicks(24, fps), fps)).toBe("00:00:01:00");
    expect(formatTimecode(frameToFlicks(24 * 3600 + 24 * 62 + 3, fps), fps)).toBe("01:01:02:03");
    expect(formatTimecode(-5, fps)).toBe("00:00:00:00");
  });

  it("formats drop-frame timecode at 29.97 and 59.94", () => {
    const f30 = { num: 30000, den: 1001 };
    expect(formatTimecode(frameToFlicks(1800, f30), f30, true)).toBe("00:01:00;02");
    expect(formatTimecode(frameToFlicks(17982, f30), f30, true)).toBe("00:10:00;00");
    expect(formatTimecode(frameToFlicks(1800, f30), f30, false)).toBe("00:01:00:00");
    const f60 = { num: 60000, den: 1001 };
    expect(formatTimecode(frameToFlicks(3600, f60), f60, true)).toBe("00:01:00;04");
  });

  it("ignores the drop-frame flag for rates that cannot drop", () => {
    const fps = { num: 25, den: 1 };
    expect(formatTimecode(frameToFlicks(25, fps), fps, true)).toBe("00:00:01:00");
  });
});
