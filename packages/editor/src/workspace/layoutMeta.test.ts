// SPDX-License-Identifier: GPL-3.0-or-later
import type { SerializedDockview } from "dockview-react";
import { describe, expect, it } from "vitest";
import {
  dockviewPart,
  EMPTY_META,
  layoutProblem,
  readLayoutMeta,
  type SavedLayout,
  spotInLayout,
  withLayoutMeta,
} from "./layoutMeta.ts";

const leaf = (id: string, size: number, views: string[] = [id]) => ({
  type: "leaf",
  size,
  data: { id: `g-${id}`, views, activeView: views[0] },
});

/** The default shape: [ [assets | preview | inspector] over timeline ]. */
const layout = {
  grid: {
    width: 1000,
    height: 800,
    orientation: "VERTICAL",
    root: {
      type: "branch",
      size: 800,
      data: [
        {
          type: "branch",
          size: 500,
          data: [leaf("assets", 200), leaf("preview", 560), leaf("inspector", 240)],
        },
        leaf("timeline", 300),
      ],
    },
  },
  panels: {
    assets: { id: "assets" },
    preview: { id: "preview" },
    inspector: { id: "inspector" },
    timeline: { id: "timeline" },
  },
} as unknown as SerializedDockview;

describe("spotInLayout", () => {
  it("finds the neighbour a panel shares a split with, and its size", () => {
    expect(spotInLayout(layout, "inspector")).toEqual({
      ref: "preview",
      direction: "right",
      size: 240,
    });
    expect(spotInLayout(layout, "assets")).toEqual({
      ref: "preview",
      direction: "left",
      size: 200,
    });
    expect(spotInLayout(layout, "preview")).toEqual({
      ref: "inspector",
      direction: "left",
      size: 560,
    });
  });

  it("docks a full-width panel at the dock's edge, not beside one panel of the row above", () => {
    expect(spotInLayout(layout, "timeline")).toEqual({
      ref: null,
      direction: "below",
      size: 300,
    });
  });

  it("says `within` for a panel sharing a tab group", () => {
    const tabbed = structuredClone(layout) as unknown as {
      grid: { root: { data: { data: unknown[] }[] } };
    };
    const row = tabbed.grid.root.data[0] as { data: { data: { views: string[] } }[] };
    row.data[2] = leaf("inspector", 240, ["inspector", "preview"]) as never;
    expect(spotInLayout(tabbed as unknown as SerializedDockview, "inspector")).toEqual({
      ref: "preview",
      direction: "within",
    });
  });

  it("is null for a panel that is not in the grid", () => {
    expect(spotInLayout(layout, "ghost")).toBeNull();
  });
});

describe("layout metadata", () => {
  const meta = {
    v: 1 as const,
    closed: ["inspector"],
    lastDock: { inspector: { ref: "preview", direction: "right" as const, size: 240 } },
  };

  it("round-trips through a saved layout and stays out of dockview's part", () => {
    const saved = withLayoutMeta(layout, meta);
    expect(readLayoutMeta(saved)).toEqual(meta);
    expect("cutweave" in dockviewPart(saved)).toBe(false);
    expect(dockviewPart(saved)).toEqual(layout);
  });

  it("leaves no block behind when there is nothing to remember", () => {
    expect("cutweave" in withLayoutMeta(layout, EMPTY_META)).toBe(false);
    const again = withLayoutMeta(withLayoutMeta(layout, meta), EMPTY_META);
    expect("cutweave" in again).toBe(false);
  });

  it("reads nothing from a layout of another version or a damaged block", () => {
    expect(readLayoutMeta({ ...layout, cutweave: { v: 99, closed: ["x"] } })).toEqual(EMPTY_META);
    expect(readLayoutMeta({ ...layout, cutweave: "nope" })).toEqual(EMPTY_META);
    expect(readLayoutMeta(null)).toEqual(EMPTY_META);
    const damaged = readLayoutMeta({
      ...layout,
      cutweave: {
        v: 1,
        closed: ["a", 7, "a"],
        lastDock: { a: { ref: 4, direction: "left" }, b: { ref: null, direction: "up" }, c: null },
      },
    } as SavedLayout);
    expect(damaged.closed).toEqual(["a"]);
    expect(damaged.lastDock).toEqual({});
  });
});

describe("layoutProblem", () => {
  const known = new Set(["assets", "preview", "inspector", "timeline"]);

  it("accepts a good layout", () => {
    expect(layoutProblem(layout, known)).toBeNull();
  });

  it("names why a damaged one cannot be restored", () => {
    expect(layoutProblem("nope", known)).not.toBeNull();
    expect(layoutProblem({ grid: {}, panels: {} }, known)).not.toBeNull();
    expect(layoutProblem({ ...layout, panels: 3 }, known)).not.toBeNull();
    expect(layoutProblem({ ...layout, panels: { ...layout.panels, ghost: {} } }, known)).toMatch(
      /ghost/,
    );
    const missing = structuredClone(layout) as unknown as { panels: Record<string, unknown> };
    delete missing.panels.timeline;
    expect(layoutProblem(missing, known)).toMatch(/timeline/);
  });

  describe("with detached panels' windows (WP-2H.14)", () => {
    /** The Inspector in its own window; its old group stays hidden and empty in the grid. */
    const popout = (extra: Record<string, unknown> = {}) => ({
      data: { id: "7", views: ["inspector"], activeView: "inspector" },
      gridReferenceGroup: "g-inspector",
      position: { left: 1200, top: 80, width: 320, height: 600 },
      url: "/popout.html",
      ...extra,
    });
    const withPopouts = (popoutGroups: unknown) => ({ ...layout, popoutGroups });

    it("accepts one, and a layout saved before there were any", () => {
      expect(layoutProblem(withPopouts([popout()]), known)).toBeNull();
      expect(layoutProblem(withPopouts([popout({ position: null, url: undefined })]), known)).toBe(
        null,
      );
      expect(layoutProblem(withPopouts([]), known)).toBeNull();
      expect("popoutGroups" in layout).toBe(false);
      expect(layoutProblem(layout, known)).toBeNull();
    });

    it("accepts a window holding a nested grid of groups", () => {
      const grid = {
        root: { type: "branch", data: [leaf("inspector", 300), leaf("assets", 300)] },
        orientation: "HORIZONTAL",
        width: 600,
        height: 400,
      };
      expect(layoutProblem(withPopouts([{ grid, url: "/popout.html" }]), known)).toBeNull();
    });

    it("checks its panels like the grid's, and its page and position", () => {
      const ghost = popout({ data: { id: "7", views: ["ghost"] } });
      expect(layoutProblem(withPopouts([ghost]), known)).toBe("no state for ghost");
      expect(layoutProblem(withPopouts([popout({ url: "https://example.com/" })]), known)).toBe(
        "bad popout page",
      );
      expect(layoutProblem(withPopouts([popout({ url: "/index.html" })]), known)).toBe(
        "bad popout page",
      );
      expect(layoutProblem(withPopouts([popout({ position: { left: "x", top: 0 } })]), known)).toBe(
        "bad popout position",
      );
      expect(layoutProblem(withPopouts([{ url: "/popout.html" }]), known)).toBe("bad popout group");
      expect(layoutProblem(withPopouts("nope"), known)).toBe("bad popout groups");
      expect(layoutProblem(withPopouts([7]), known)).toBe("bad popout group");
    });

    it("keeps them through the layout's own block", () => {
      const saved = withLayoutMeta(withPopouts([popout()]) as unknown as SerializedDockview, {
        ...EMPTY_META,
        lastDock: { inspector: { ref: "preview", direction: "right", size: 240 } },
      });
      expect(dockviewPart(saved).popoutGroups).toEqual([popout()]);
      expect(readLayoutMeta(saved).lastDock.inspector).toEqual({
        ref: "preview",
        direction: "right",
        size: 240,
      });
    });
  });
});
