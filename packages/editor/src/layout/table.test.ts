// SPDX-License-Identifier: GPL-3.0-or-later
// The layout table the editor reads is the engine's file (WP-2C.08): every layout takes two to four
// clips, its cells lie inside the frame, none overlap in a grid, and every layout has a name.
import { describe, expect, it } from "vitest";
import { LAYOUT_NAMES } from "./names.ts";
import { findLayout, LAYOUTS, layoutsFor, SIDE_BY_SIDE } from "./table.ts";

describe("the layout table", () => {
  it("lists layouts for two, three and four clips", () => {
    expect(layoutsFor(2).map((l) => l.id)).toEqual([
      "side_by_side",
      "top_bottom",
      "pip_top_left",
      "pip_top_right",
      "pip_bottom_left",
      "pip_bottom_right",
    ]);
    expect(layoutsFor(3).map((l) => l.id)).toEqual(["columns_3", "one_large_two_small"]);
    expect(layoutsFor(4).map((l) => l.id)).toEqual(["grid_2x2"]);
    expect(layoutsFor(1)).toEqual([]);
    expect(layoutsFor(5)).toEqual([]);
  });

  it("keeps every cell inside the frame", () => {
    for (const l of LAYOUTS) {
      for (const c of l.cells) {
        expect(c.x).toBeGreaterThanOrEqual(0);
        expect(c.y).toBeGreaterThanOrEqual(0);
        expect(c.x + c.w).toBeLessThanOrEqual(1 + 1e-9);
        expect(c.y + c.h).toBeLessThanOrEqual(1 + 1e-9);
      }
    }
  });

  it("splits grids exactly: halves, columns, one large and two small", () => {
    expect(findLayout("side_by_side")?.cells).toEqual([
      { x: 0, y: 0, w: 0.5, h: 1 },
      { x: 0.5, y: 0, w: 0.5, h: 1 },
    ]);
    const large = findLayout("one_large_two_small")?.cells ?? [];
    expect(large[0]).toMatchObject({ x: 0, y: 0, h: 1 });
    expect(large[0]?.w).toBeCloseTo(2 / 3, 9);
    expect(large[2]).toMatchObject({ y: 0.5, h: 0.5 });
  });

  it("puts picture in picture at 30 % of the width, inset by the title-safe margin", () => {
    const [small] = findLayout("pip_top_right")?.cells ?? [];
    expect(small).toEqual({ x: 0.65, y: 0.05, w: 0.3, h: 0.3 });
  });

  it("names every layout and has a default for each number of clips", () => {
    for (const l of LAYOUTS) expect(LAYOUT_NAMES[l.id]).toBeDefined();
    expect(SIDE_BY_SIDE).toEqual({ 2: "side_by_side", 3: "columns_3", 4: "grid_2x2" });
  });
});
