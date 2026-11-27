// SPDX-License-Identifier: GPL-3.0-or-later
// The transport bar's collapse order and the level that fits a width (#225).
import { describe, expect, it } from "vitest";
import { COLLAPSE_STEPS, collapsedAt, fitLevel, MAX_LEVEL, recordNeed } from "./barLayout.ts";

describe("collapsedAt", () => {
  it("moves nothing at level 0 and everything that can move at the last level", () => {
    expect([...collapsedAt(0)]).toEqual([]);
    expect([...collapsedAt(MAX_LEVEL)].sort()).toEqual(
      [
        "fullscreen",
        "loop",
        "mute",
        "proxies",
        "quality",
        "source",
        "stepBack",
        "stepForward",
        "toEnd",
        "toStart",
        "volume",
      ].sort(),
    );
    expect([...collapsedAt(MAX_LEVEL + 3)]).toHaveLength(11);
    expect([...collapsedAt(-1)]).toEqual([]);
  });

  it("drops the least important first, the frame steps last", () => {
    expect([...collapsedAt(1)]).toEqual(["fullscreen", "source"]);
    expect([...collapsedAt(2)]).toEqual(["fullscreen", "source", "quality", "proxies"]);
    expect(collapsedAt(3).has("volume")).toBe(true);
    expect(collapsedAt(3).has("loop")).toBe(false);
    expect(collapsedAt(4).has("loop")).toBe(true);
    expect(collapsedAt(4).has("mute")).toBe(false);
    expect(collapsedAt(5).has("toStart")).toBe(true);
    expect(collapsedAt(5).has("stepBack")).toBe(false);
    expect(collapsedAt(6).has("stepForward")).toBe(true);
    expect(MAX_LEVEL).toBe(6);
  });

  it("every level adds to the one before, and no control is in two steps", () => {
    const all = COLLAPSE_STEPS.flat();
    expect(new Set(all).size).toBe(all.length);
    for (let level = 1; level <= MAX_LEVEL; level += 1) {
      for (const control of collapsedAt(level - 1)) {
        expect(collapsedAt(level).has(control)).toBe(true);
      }
    }
  });
});

describe("fitLevel", () => {
  const needs = [900, 780, 640, 430, 380, 280, 200];

  it("is the first level that fits", () => {
    expect(fitLevel(1200, needs)).toBe(0);
    expect(fitLevel(900, needs)).toBe(0);
    expect(fitLevel(899, needs)).toBe(1);
    expect(fitLevel(700, needs)).toBe(2);
    expect(fitLevel(480, needs)).toBe(3);
    expect(fitLevel(400, needs)).toBe(4);
    expect(fitLevel(300, needs)).toBe(5);
    expect(fitLevel(240, needs)).toBe(6);
  });

  it("is the last level when nothing fits", () => {
    expect(fitLevel(100, needs)).toBe(MAX_LEVEL);
    expect(fitLevel(0, needs)).toBe(MAX_LEVEL);
  });

  it("treats widths within half a pixel as equal", () => {
    expect(fitLevel(899.6, needs)).toBe(0);
    expect(fitLevel(899.4, needs)).toBe(1);
  });

  it("tries a level that has not been measured", () => {
    expect(fitLevel(500, [])).toBe(0);
    expect(fitLevel(500, [900])).toBe(1);
    expect(fitLevel(500, [900, 780, 640])).toBe(3);
    // Wider again: level 1 is known to fit, so the bar goes straight back to it.
    expect(fitLevel(800, [900, 780, 640, 430])).toBe(1);
  });

  it("settles by measuring one level at a time", () => {
    // What each level really needs; the bar learns it only by showing the level.
    let known: (number | undefined)[] = [];
    let level = 0;
    for (let i = 0; i <= MAX_LEVEL; i += 1) {
      known = recordNeed(known, level, needs[level] ?? 0);
      const next = fitLevel(400, known);
      if (next === level) break;
      level = next;
    }
    expect(level).toBe(4);
    expect(known).toEqual([900, 780, 640, 430, 380]);
  });
});

describe("recordNeed", () => {
  it("keeps what the other levels needed while this one needs the same", () => {
    expect(recordNeed([900, 780], 2, 640)).toEqual([900, 780, 640]);
    expect(recordNeed([900, 780, 640], 2, 640.3)).toEqual([900, 780, 640.3]);
  });

  it("forgets the other levels when this one changed size (density, language, time display)", () => {
    const next = recordNeed([900, 780, 640], 2, 700);
    expect(next[2]).toBe(700);
    expect(next[0]).toBeUndefined();
    expect(next[1]).toBeUndefined();
    // So the bar tries level 0 again and measures its way back.
    expect(fitLevel(2000, next)).toBe(0);
  });

  it("does not change the list it was given", () => {
    const before = [900, 780];
    recordNeed(before, 1, 500);
    expect(before).toEqual([900, 780]);
  });
});
