// SPDX-License-Identifier: GPL-3.0-or-later
// Filmstrip tiles of sped, reversed and frozen clips (WP-2D.01).
import { describe, expect, it } from "vitest";
import { tileSpans } from "./renderer.ts";

const D = 100; // flicks per media tile

describe("tileSpans", () => {
  it("lays media tiles out at normal speed", () => {
    expect(tileSpans({ mediaId: "m", sourceIn: 150 }, 300, D, 0, 300)).toEqual([
      { j: 1, at: -50, span: 100 },
      { j: 2, at: 50, span: 100 },
      { j: 3, at: 150, span: 100 },
      { j: 4, at: 250, span: 100 },
    ]);
  });

  it("squeezes tiles at 2x and stretches them at half speed", () => {
    const fast = tileSpans({ mediaId: "m", sourceIn: 0, speed: 2 }, 100, D, 0, 100);
    expect(fast.map((t) => [t.j, t.at, t.span])).toEqual([
      [0, 0, 50],
      [1, 50, 50],
      [2, 100, 50],
    ]);
    const slow = tileSpans({ mediaId: "m", sourceIn: 0, speed: 0.5 }, 400, D, 0, 400);
    expect(slow.map((t) => [t.j, t.at])).toEqual([
      [0, 0],
      [1, 200],
      [2, 400],
    ]);
  });

  it("runs tiles from the end of the range when reversed", () => {
    // Range [0, 300): the left edge shows media 300, so tile 2 comes first.
    const back = tileSpans({ mediaId: "m", sourceIn: 0, reverse: true }, 300, D, 0, 300);
    expect(back.map((t) => [t.j, t.at])).toEqual([
      [0, 200],
      [1, 100],
      [2, 0],
      [3, -100],
    ]);
  });

  it("repeats the frozen frame's tile", () => {
    const frozen = tileSpans({ mediaId: "m", sourceIn: 250, speed: 0 }, 300, D, 0, 300);
    expect(frozen.map((t) => [t.j, t.at])).toEqual([
      [2, 0],
      [2, 100],
      [2, 200],
    ]);
  });
});
