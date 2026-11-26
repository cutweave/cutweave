// SPDX-License-Identifier: GPL-3.0-or-later
// The picture menu's arguments (WP-2H.06): Fit, Fill, Center and the flips, as `item.set_props`.
import { describe, expect, it } from "vitest";
import { sampleProject } from "../../testing/fixtures.ts";
import { flipArgs, placeArgs } from "./placement.ts";

const project = (visual: object = {}) =>
  sampleProject({
    itm_v: {
      start: 0,
      visual: {
        transform: {
          position: { x: 0.3, y: 0.1 },
          scale: { x: 0.5, y: 0.5 },
          rotation: 0,
          anchor: { x: 0.5, y: 0.5 },
        },
        opacity: 1,
        crop: { left: 0, top: 0, right: 0, bottom: 0 },
        flip: { horizontal: false, vertical: false },
        blend_mode: "normal",
        ...visual,
      },
    },
  });

describe("placeArgs", () => {
  it("Fit puts the whole picture inside the canvas, centred", () => {
    const args = placeArgs(project(), "itm_v", 0, "fit");
    expect(args?.props.transform).toMatchObject({
      scale: { x: 1, y: 1 },
      position: { x: 0, y: 0 },
    });
    expect(args?.at).toBe(0);
    expect(args?.item).toBe("itm_v");
  });

  it("Fill covers the canvas when the picture's shape differs", () => {
    // A crop to the left half is a 4:9 picture: Fill scales it to the canvas width.
    const p = project({ crop: { left: 0, top: 0, right: 0.5, bottom: 0 } });
    const args = placeArgs(p, "itm_v", 0, "fill");
    expect(args?.props.transform?.scale?.x).toBeCloseTo(2, 6);
    // Its visible half is centred: the whole picture (anchor at its centre) sits a canvas width to the right of the centre.
    expect(args?.props.transform?.position?.x).toBeCloseTo(0.5, 6);
  });

  it("Center keeps the scale", () => {
    const args = placeArgs(project(), "itm_v", 0, "center");
    expect(args?.props.transform?.scale).toEqual({ x: 0.5, y: 0.5 });
    expect(args?.props.transform?.position).toEqual({ x: 0, y: 0 });
  });

  it("is null for a clip with no picture at the playhead", () => {
    expect(placeArgs(project(), "itm_v", 100 * 705_600_000, "fit")).toBeNull();
    expect(placeArgs(null, "itm_v", 0, "fit")).toBeNull();
  });
});

describe("flipArgs", () => {
  it("mirrors every clip the opposite way of the first", () => {
    const args = flipArgs(project(), ["itm_v"], 0, "horizontal");
    expect(args?.props.flip).toEqual({ horizontal: true, vertical: null });
    expect(args?.items).toEqual(["itm_v"]);
    const flipped = flipArgs(
      project({ flip: { horizontal: true, vertical: false } }),
      ["itm_v"],
      0,
      "horizontal",
    );
    expect(flipped?.props.flip?.horizontal).toBe(false);
  });
});
